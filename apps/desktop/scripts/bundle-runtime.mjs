// Builds the bundled runtime consumed by the Tauri shell:
//
//   apps/desktop/runtime/
//     node/                         Node executable (node.exe / node)
//     dsh/                          pnpm-deploy closure of @deepseek-ai/dsh
//                                   (+ @deepseek-ai/dsh-web-frontend/dist, pnpm)
//     plugins/dsh-win-terminal-inspector/   local Windows-only plugin
//
// The shell resolves these paths in src-tauri/src/dsh_runner.rs
// (resolve_runtime). Run from apps/desktop:
//
//   node scripts/bundle-runtime.mjs [--platform win|mac|linux] [--node-version v22.20.0]
//
// --platform defaults to the host platform. In CI the workflow passes the
// runner platform so each matrix job bundles its own runtime.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, cpSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { createWriteStream } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { readFileSync, statSync, realpathSync } from 'node:fs';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const RUNTIME = join(ROOT, 'runtime');
const NODE_DIR = join(RUNTIME, 'node');
const DSH_DIR = join(RUNTIME, 'dsh');
const PLUGINS_DIR = join(RUNTIME, 'plugins');

const args = process.argv.slice(2);
const platformArg = args.find((a) => a.startsWith('--platform='))?.split('=')[1];
const nodeArg = args.find((a) => a.startsWith('--node-version='))?.split('=')[1];

const PLATFORM = platformArg ?? hostPlatform();
const DSH = '@deepseek-ai/dsh';

function hostPlatform() {
  if (process.platform === 'win32') return 'win';
  if (process.platform === 'darwin') return 'mac';
  return 'linux';
}

// execFileSync on Windows cannot resolve `.cmd` shims (corepack's pnpm)
// without shell interpretation; route through cmd.exe there.
function runPnpm(args, opts) {
  if (process.platform === 'win32') {
    execFileSync('cmd.exe', ['/c', 'pnpm', ...args], { ...opts, stdio: 'inherit' });
  } else {
    execFileSync('pnpm', args, { ...opts, stdio: 'inherit' });
  }
}

function step(msg) {
  console.log(`[bundle-runtime] ${msg}`);
}

async function latestLtsV22() {
  const res = await fetch('https://nodejs.org/dist/index.json');
  if (!res.ok) throw new Error(`nodejs.org index: HTTP ${res.status}`);
  const releases = await res.json();
  const v22 = releases.find((r) => r.lts !== false && r.version.startsWith('v22.'));
  if (!v22) throw new Error('no LTS v22 release found');
  return v22.version;
}

function nodeArchive(version, platform, arch) {
  const base = `node-${version}-${platform}-${arch}`;
  const url = `https://nodejs.org/dist/${version}/${base}`;
  if (platform === 'win') return { name: `${base}.zip`, url: `${url}.zip` };
  return { name: `${base}.tar.xz`, url: `${url}.tar.xz` };
}

async function download(url, dest) {
  step(`download ${url}`);
  const res = await fetch(url);
  if (!res.ok) throw new Error(`download ${url}: HTTP ${res.status}`);
  const out = createWriteStream(dest);
  for await (const chunk of res.body) {
    if (!out.write(chunk)) {
      await new Promise((resolve) => out.once('drain', resolve));
    }
  }
  await new Promise((resolve, reject) => {
    out.on('finish', resolve);
    out.on('error', reject);
    out.end();
  });
}

function extract(archive, dest) {
  mkdirSync(dest, { recursive: true });
  step(`extract ${archive}`);
  if (archive.endsWith('.zip')) {
    // Windows ships bsdtar which reads zip archives; PowerShell's
    // Expand-Archive is not available in every environment.
    execFileSync('tar', ['-xf', archive, '-C', dest], { stdio: 'inherit' });
  } else {
    execFileSync('tar', ['-xJf', archive, '-C', dest], { stdio: 'inherit' });
  }
}

function findNodeDir(dir) {
  const top = readdirSync(dir).find((name) => name.startsWith('node-v'));
  if (!top) throw new Error(`no node-* directory inside ${dir}`);
  const base = join(dir, top);
  const exe = PLATFORM === 'win'
    ? join(base, 'node.exe')
    : join(base, 'bin', 'node');
  if (!existsSync(exe)) throw new Error(`missing ${exe}`);
  return base;
}

async function installNode() {
  const arch = PLATFORM === 'mac' ? 'arm64' : 'x64';
  const version = nodeArg ?? (await latestLtsV22());
  const os = PLATFORM === 'win' ? 'win' : PLATFORM === 'mac' ? 'darwin' : 'linux';
  const archiveInfo = nodeArchive(version, os, arch);
  const tmp = join(RUNTIME, '.tmp');
  mkdirSync(tmp, { recursive: true });
  const archive = join(tmp, archiveInfo.name);
  await download(archiveInfo.url, archive);
  extract(archive, tmp);
  rmSync(NODE_DIR, { recursive: true, force: true });
  mkdirSync(NODE_DIR, { recursive: true });
  // Copy the whole distribution (node binary + bundled npm) so the shell can
  // install plugins without an external toolchain.
  cpSync(findNodeDir(tmp), NODE_DIR, { recursive: true });
  rmSync(tmp, { recursive: true, force: true });
  step(`node ${version} installed`);
}

async function deployDsh() {
  const repo = resolve(ROOT, '..', '..');
  if (!existsSync(join(repo, 'pnpm-workspace.yaml'))) {
    throw new Error(`repository root not found at ${repo}`);
  }
  step(`pnpm deploy ${DSH} into ${DSH_DIR}`);
  rmSync(DSH_DIR, { recursive: true, force: true });
  // --legacy: this workspace has no inject-workspace-packages. Peers are
  // required at runtime (npm's auto-install-peers behaviour), so force them
  // explicitly — pnpm deploy otherwise drops peer-only @deepseek-ai/* rows.
  runPnpm(
    ['deploy', DSH_DIR, '--filter', DSH, '--prod', '--legacy', '--config.auto-install-peers=true'],
    { cwd: repo },
  );

  const distSrc = join(repo, 'apps', 'web', 'dist');
  if (!existsSync(distSrc)) {
    throw new Error('apps/web/dist missing; run `pnpm run build` from the repository root first');
  }
  const distDest = join(DSH_DIR, 'node_modules', '@deepseek-ai', 'dsh-web-frontend', 'dist');
  mkdirSync(distDest, { recursive: true });
  cpSync(distSrc, distDest, { recursive: true });
  step('web frontend dist copied into deployed closure');

  // deploy used --ignore-scripts (the repo root postinstall needs lefthook);
  // run the native installs we actually need inside the closure. node-pty's
  // prebuild.js downloads its prebuilt binary; koffi/better-sqlite3 ship
  // per-platform prebuilt npm packages.
  step('rebuild native modules in closure');
  runPnpm(['rebuild', 'node-pty'], { cwd: DSH_DIR });

  // Ship pnpm so `dsh plugin` and the market can install/update plugins
  // inside the desktop's web profile. Pin the same major as the deploy step
  // (store format must match) and install into a scratch prefix first —
  // `npm install --prefix <nodeDir>` would drop the bundled npm itself.
  const nodeExe = join(NODE_DIR, PLATFORM === 'win' ? 'node.exe' : 'node');
  const npmCli = join(NODE_DIR, 'node_modules', 'npm', 'bin', 'npm-cli.js');
  if (!existsSync(npmCli)) {
    throw new Error(`bundled npm missing at ${npmCli}; node distribution incomplete`);
  }
  step('install pnpm into runtime');
  const pnpmVersion = '11.7.0'; // must match the pnpm used by pnpm deploy
  const scratch = join(RUNTIME, '.tmp', 'pnpm-prefix');
  rmSync(scratch, { recursive: true, force: true });
  mkdirSync(scratch, { recursive: true });
  execFileSync(nodeExe, [npmCli, 'install', '--no-save', '--no-audit', '--no-fund', '--prefix', scratch, `pnpm@${pnpmVersion}`], { stdio: 'inherit' });
  const pnpmPkg = join(scratch, 'node_modules', 'pnpm');
  if (!existsSync(pnpmPkg)) {
    throw new Error('pnpm did not install into scratch prefix');
  }
  rmSync(join(NODE_DIR, 'node_modules', 'pnpm'), { recursive: true, force: true });
  cpSync(pnpmPkg, join(NODE_DIR, 'node_modules', 'pnpm'), { recursive: true });
  rmSync(scratch, { recursive: true, force: true });
  step(`pnpm@${pnpmVersion} installed`);

  flattenClosure();
  patchBrokenLinks(repo);
  patchMissingPeers(repo);
}

// pnpm deploy (legacy) does not install peer-only packages: rows that only
// appear in some package's peerDependencies get no .pnpm store entry and no
// symlink. Their imports then fail at runtime. Detect every non-optional
// peer with neither a store instance nor a top-level entry, and copy those
// from the repository's packages/ (and vendor/) tree into the closure's
// top-level node_modules. Packages that DO have a store instance keep their
// pnpm store resolution — hoisting everything would break third-party
// dependency lookup (e.g. zod) instead.
function copyPkgContents(src, dest) {
  if (!existsSync(src)) return;
  // The deploy step may have left a dangling link at dest; remove it first so
  // the copy writes a real directory (cpSync would otherwise follow it).
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dirname(dest), { recursive: true });
  // dereference: store entries are symlinks; copy the contents, not the link
  // (a copied relative link would dangle at the top-level location). Drop
  // type declarations, source maps, tests and docs: the runtime never reads
  // them, they bloat the bundle, and type-declaration file names exceed
  // Windows' path limit inside the installer.
  cpSync(src, dest, {
    recursive: true,
    dereference: true,
    filter: (s) => {
      const parts = s.split(/[\\/]/);
      const last = parts.pop();
      if (last === 'node_modules') return false;
      if (last === 'test' || last === 'tests' || last === 'docs' || last === 'coverage' || last === '.github') return false;
      if (last.endsWith('.d.ts') || last.endsWith('.d.cts') || last.endsWith('.d.mts') || last.endsWith('.map')) return false;
      return true;
    },
  });
}

// The web profile's loader resolves every bundle/entry row from the profile
// directory via ESM, which never searches a pnpm virtual store. The deployed
// closure must therefore look like a traditional node_modules: flatten every
// package from .pnpm/*/node_modules into the top-level node_modules (the
// profile gets a junction to it, see dsh_runner.rs). copyPkgContents strips
// each package's own node_modules, so links are not followed.
function flattenClosure() {
  flattenNodeModules(join(DSH_DIR, 'node_modules'));
  // The virtual store is now redundant (everything it held is at the
  // top level); dropping it halves the bundle size.
  rmSync(join(DSH_DIR, 'node_modules', '.pnpm'), { recursive: true, force: true });
  // @mistralai (optional pi-ai provider dep) ships file names that exceed
  // Windows' path limit inside the NSIS installer; DeepSeek/other providers
  // never load it, so drop the whole package.
  rmSync(join(DSH_DIR, 'node_modules', '@mistralai'), { recursive: true, force: true });
  step('removed .pnpm virtual store and @mistralai');
}

// Turn a pnpm-style node_modules (virtual store + top-level links) into a
// traditional flat layout, so plugins copied to the user profile resolve
// their dependencies from the top level.
function flattenNodeModules(topNm) {
  const pnpmStore = join(topNm, '.pnpm');
  const isDir = (p) => {
    try {
      return statSync(p).isDirectory();
    } catch {
      return false;
    }
  };
  let copied = 0;
  if (!existsSync(pnpmStore)) {
    return;
  }
  for (const store of readdirSync(pnpmStore, { withFileTypes: true })
    .filter((d) => d.isDirectory())
    .map((d) => d.name)) {
    const nm = join(pnpmStore, store, 'node_modules');
    if (!existsSync(nm)) continue;
    for (const entry of readdirSync(nm, { withFileTypes: true })) {
      if (entry.name === '.bin') continue;
      const full = join(nm, entry.name);
      // Store entries are symlinks; follow them via stat.
      if (entry.name.startsWith('@') && isDir(full)) {
        for (const sub of readdirSync(full, { withFileTypes: true })) {
          const subFull = join(full, sub.name);
          if (!isDir(subFull)) continue;
          copyPkgContents(subFull, join(topNm, entry.name, sub.name));
          copied += 1;
        }
      } else if (isDir(full) || entry.isFile()) {
        copyPkgContents(full, join(topNm, entry.name));
        copied += 1;
      }
    }
  }
  step(`flattened ${copied} packages into ${topNm}`);
}

// pnpm deploy (legacy) leaves dangling symlinks for dependency-chain rows it
// failed to install (e.g. dsh-base's dependencies dsh-llm/dsh-session have no
// store instance). Any link whose realpath is missing gets its package copied
// from the repository into the top-level node_modules.
function patchBrokenLinks(repo) {
  const pnpmStore = join(DSH_DIR, 'node_modules', '.pnpm');
  const topNm = join(DSH_DIR, 'node_modules');
  const pkgMap = buildPkgMap(repo);
  let fixed = 0;
  if (!existsSync(pnpmStore)) return;
  for (const store of readdirSync(pnpmStore, { withFileTypes: true })
    .filter((d) => d.isDirectory())
    .map((d) => d.name)) {
    const nm = join(pnpmStore, store, 'node_modules');
    if (!existsSync(nm)) continue;
    for (const entry of readdirSync(nm, { withFileTypes: true })) {
      const full = join(nm, entry.name);
      const names = entry.name.startsWith('@')
        ? readdirSync(full, { withFileTypes: true }).map((s) => `${entry.name}/${s.name}`)
        : [entry.name];
      for (const name of names) {
        const linkPath = join(nm, ...name.split('/'));
        try {
          realpathSync(linkPath);
          continue; // link resolves
        } catch {
          // dangling
        }
        const dest = join(topNm, ...name.split('/'));
        if (existsSync(dest)) continue;
        const src = pkgMap.get(name);
        if (!src) {
          step(`WARN: dangling link ${name} has no repository package`);
          continue;
        }
        copyPkgContents(src, dest);
        fixed += 1;
        step(`patched broken link ${name}`);
      }
    }
  }
  step(`broken links patched: ${fixed}`);
}

function patchMissingPeers(repo) {
  const pnpmStore = join(DSH_DIR, 'node_modules', '.pnpm');
  const topNm = join(DSH_DIR, 'node_modules');
  const storeNames = existsSync(pnpmStore)
    ? new Set(readdirSync(pnpmStore, { withFileTypes: true })
        .filter((d) => d.isDirectory())
        .map((d) => d.name))
    : new Set();

  const missing = new Map();
  const allPkgs = [];
  // pnpm store entries are symlinks that can form cycles; walk real paths and
  // visit each once.
  const seen = new Set();
  const walk = (dir) => {
    let entries = [];
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const full = join(dir, entry.name);
      let real;
      let isDir = false;
      try {
        real = realpathSync(full);
        isDir = statSync(real).isDirectory();
      } catch {
        continue;
      }
      if (isDir) {
        if (seen.has(real)) continue;
        seen.add(real);
        walk(real);
      } else if (entry.name === 'package.json') {
        allPkgs.push(real);
      }
    }
  };
  walk(join(DSH_DIR, 'node_modules'));

  for (const pkgPath of allPkgs) {
    let pkg;
    try {
      pkg = JSON.parse(readFileSync(pkgPath, 'utf8'));
    } catch {
      continue;
    }
    const peers = pkg.peerDependencies ?? {};
    const meta = pkg.peerDependenciesMeta ?? {};
    for (const name of Object.keys(peers)) {
      if (meta[name]?.optional) continue;
      if (existsSync(join(topNm, ...name.split('/')))) continue;
      const prefix = name.startsWith('@') ? `${name.replace('/', '+')}@` : `${name}@`;
      if ([...storeNames].some((s) => s.startsWith(prefix))) continue;
      if (!missing.has(name)) missing.set(name, pkg.name ?? pkgPath);
    }
  }

  const pkgMap = buildPkgMap(repo);
  let patched = 0;
  for (const [name, requiredBy] of missing) {
    const src = pkgMap.get(name);
    if (!src) {
      step(`WARN: peer ${name} (required by ${requiredBy}) has no repository package`);
      continue;
    }
    copyPkgContents(src, join(topNm, ...name.split('/')));
    patched += 1;
    step(`patched peer ${name}`);
  }
  step(`missing peers patched: ${patched}`);
}

function buildPkgMap(repo) {
  const map = new Map();
  const register = (pj) => {
    if (!existsSync(pj)) return;
    try {
      const pkg = JSON.parse(readFileSync(pj, 'utf8'));
      if (pkg.name) map.set(pkg.name, dirname(pj));
    } catch {
      // unparseable manifest: skip
    }
  };
  // packages/<group>/<pkg>/package.json
  const pkgsRoot = join(repo, 'packages');
  let groups = [];
  try {
    groups = readdirSync(pkgsRoot, { withFileTypes: true })
      .filter((d) => d.isDirectory())
      .map((d) => d.name);
  } catch {
    return map;
  }
  for (const g of groups) {
    const dir = join(pkgsRoot, g);
    let pkgs = [];
    try {
      pkgs = readdirSync(dir, { withFileTypes: true })
        .filter((d) => d.isDirectory())
        .map((d) => d.name);
    } catch {
      continue;
    }
    for (const pkg of pkgs) register(join(dir, pkg, 'package.json'));
  }
  // vendor/<name>/package.json (vendored Cordis, rescoped to @deepseek-ai/*)
  const vendorRoot = join(repo, 'vendor');
  let vendored = [];
  try {
    vendored = readdirSync(vendorRoot, { withFileTypes: true })
      .filter((d) => d.isDirectory())
      .map((d) => d.name);
  } catch {
    return map;
  }
  for (const name of vendored) register(join(vendorRoot, name, 'package.json'));
  return map;
}

async function installWinTerminalInspector() {
  if (PLATFORM !== 'win') return;
  const dest = join(PLUGINS_DIR, 'dsh-win-terminal-inspector');
  rmSync(dest, { recursive: true, force: true });
  // codeload.github.com is reachable from build sandboxes where github.com's
  // git port is not; a tarball also avoids shipping a nested .git.
  const url = 'https://codeload.github.com/clearkurt/dsh-win-terminal-inspector/tar.gz/refs/heads/main';
  const archive = join(RUNTIME, '.wti.tar.gz');
  await download(url, archive);
  const tmp = join(PLUGINS_DIR, '.wti-tmp');
  rmSync(tmp, { recursive: true, force: true });
  mkdirSync(tmp, { recursive: true });
  execFileSync('tar', ['-xzf', archive, '-C', tmp], { stdio: 'inherit' });
  const top = readdirSync(tmp).find((name) => name.startsWith('dsh-win-terminal-inspector-'));
  if (!top) throw new Error(`unexpected tarball layout in ${tmp}`);
  cpSync(join(tmp, top), dest, { recursive: true });
  rmSync(tmp, { recursive: true, force: true });
  rmSync(archive, { force: true });
  step('dsh-win-terminal-inspector installed');
}

function writeManifest() {
  const manifest = {
    platform: PLATFORM,
    node: readdirSync(NODE_DIR).join(','),
    dshCli: existsSync(join(DSH_DIR, 'lib', 'bin.js')),
    plugins: existsSync(PLUGINS_DIR) ? readdirSync(PLUGINS_DIR) : [],
  };
  writeFileSync(join(RUNTIME, 'manifest.json'), JSON.stringify(manifest, null, 2));
  step(`manifest: ${JSON.stringify(manifest)}`);
}

async function main() {
  step(`platform=${PLATFORM} root=${ROOT}`);
  mkdirSync(RUNTIME, { recursive: true });
  rmSync(join(RUNTIME, '.tmp'), { recursive: true, force: true });

  const nodeComplete = existsSync(join(NODE_DIR, 'node_modules', 'npm', 'bin', 'npm-cli.js'));
  if (nodeComplete) {
    step('node already present, skipping download');
  } else {
    await installNode();
  }
  await deployDsh();
  await installWinTerminalInspector();
  await prepareDefaultPluginDshPlugin();
  writeManifest();
}

// The plugin market ships preinstalled: install its full dependency closure
// into runtime/plugins/dsh-plugin (network at bundle time, offline
// afterwards). The shell copies this directory into the web profile on first
// launch.
async function prepareDefaultPluginDshPlugin() {
  const dest = join(PLUGINS_DIR, 'dsh-plugin');
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dest, { recursive: true });
  writeFileSync(
    join(dest, 'package.json'),
    JSON.stringify({ name: 'dsh-plugin-bundle', private: true, type: 'module', dependencies: { 'dsh-plugin': 'latest' } }, null, 2),
  );
  // Isolate from the parent workspace so pnpm treats this as a standalone
  // project (no purge/TTY prompts, no lefthook postinstall). minimumReleaseAge
  // defaults on in pnpm 11 and would make `latest` skip a fresh market
  // release, silently resolving to an older version; zero it so the market
  // plugin tracks the newest published build.
  writeFileSync(join(dest, 'pnpm-workspace.yaml'), 'packages: []\nminimumReleaseAge: 0\n');
  step('install dsh-plugin closure into runtime/plugins/dsh-plugin');
  runPnpm(['install', '--prod', '--ignore-scripts', '--lockfile=false'], { cwd: dest });
  const marketPkg = join(dest, 'node_modules', 'dsh-plugin', 'package.json');
  if (!existsSync(marketPkg)) {
    throw new Error('dsh-plugin did not install into runtime/plugins/dsh-plugin');
  }
  // Flatten the closure so a copied profile plugin resolves its own deps.
  flattenNodeModules(join(dest, 'node_modules'));
  rmSync(join(dest, 'node_modules', '.pnpm'), { recursive: true, force: true });
  step('dsh-plugin closure installed');
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
