---
feature: desktop-client
status: in-progress
updated: 2026-08-26
branch: feat/desktop-plugin-hub
commits: 57da7798a..9176ccb11
---

# Desktop Client（Tauri 全平台桌面客户端）

## Report

**What was built** — 桌面端内置插件市场由 `dshmarket` 切换为 `dsh-plugin`（npm 包，对应 dshplugin/dsh-plugin-hub 插件中心）：`scripts/bundle-runtime.mjs` 在打包期把 `dsh-plugin@latest` 装进 `runtime/plugins/dsh-plugin`（`--prod --ignore-scripts`，flatten 闭包，pnpm-workspace.yaml 置 `minimumReleaseAge: 0` 保证跟踪最新发行），首启时 `setup.rs::ensure_default_plugins` 复制进 web profile 的 `plugins/` 并写入 `cordis.patch.yml` insert 行（`id: dsh-plugin`，name `./plugins/dsh-plugin/node_modules/dsh-plugin/lib/index.js`，与包 `main` 一致）。内容链接改为系统默认浏览器打开：主窗口改由 `WebviewWindowBuilder` 构建（`tauri.conf.json` `create: false`）并挂 `on_new_window`，http/https/mailto/tel 经 `tauri-plugin-opener` 外开、其余 scheme 拒绝、弹窗一律 `Deny`，壳页面、向导页与 dsh iframe 内的 `target="_blank"` / `window.open` 均不再在应用内开新窗口。首启向导可选插件默认全部不勾选（移除 `dsh-tauri` 桌面导航桥条目的 `defaultOn: true`），导航桥等可选插件改为按需勾选安装。

**Verification** — `cargo check`（apps/desktop/src-tauri）：PASS，仅 1 个存量 warning（dsh_runner.rs:385 unused import）。`node --check` scripts/bundle-runtime.mjs、ui/dist/wizard.js、ui/dist/app.js：PASS。npm 烟测：`pnpm install --prod --ignore-scripts --lockfile=false` + `minimumReleaseAge: 0` 安装 `dsh-plugin@1.3.5`，`node_modules/dsh-plugin/lib/index.js` 存在且可加载；npm 元数据确认 `main=lib/index.js`、repository=dshplugin/dsh-plugin-hub、无运行时依赖。独立评审对照 wry 0.55.1 源码确认 `on_new_window` 拦截跨源 iframe（127.0.0.1:3081）的 NewWindowRequested，`Deny` → 不产生窗口。未做：bundle-runtime.mjs 全量执行（需先 `pnpm run build` 并下载 Node 运行时，由 CI 的 desktop-release.yml 覆盖）；应用内点击的端到端手测（Windows GUI，本环境无法无头执行）。

**Journey log** — 1) 旧市场包 `dshmarket` 与 dsh-plugin-hub 无直接关系，必须先验证 npm 包 `dsh-plugin` 的 repository 字段才确认切换目标正确。2) `tauri.conf.json` 声明 `bundle.resources` 的 `../runtime` 必须存在，否则 tauri-build 的 cargo check 直接失败——worktree 里建空目录即可通过。3) worktree 提交会触发 lefthook pre-commit 的 lint 步骤，缺 node_modules 时报 `tsx: No such file or directory`，需先 `pnpm install --frozen-lockfile`。4) 评审确认 Tauri 2 (wry) 无 `on_new_window` 处理器时 WebView2 会把 `target="_blank"` 标记为已处理却不展示任何窗口——即桌面端链接"点了没反应"的根因，`Deny` + opener 外开是正解。5) setup.rs 原 `defaults` 元组第二元素从未被使用且与 insert 行内容不一致，属死代码，评审 M1 指出后移除。

## [S1] Problem

DeepSeek Harness（`dsh`）目前是 CLI + 浏览器形态：`dsh web` 在 `http://127.0.0.1:3080` 起本地服务并打开系统浏览器。用户需要一款**桌面独立客户端**：双击即用、关窗不退出（托盘常驻）、无需理解端口与浏览器标签页、安装包尽量小（选定 Tauri 而非 Electron）、全平台（Windows / macOS / Linux）发行，并由 GitHub Actions 打包 release 包。同时四个 agent preset（`standard` / `minimal` / `code` / `cordis`）的全部功能必须原样保留且可正常运行，插件安装/更新（dsh plugin、dsh-plugin 插件中心）等机制在桌面端也必须可用。

仓库形态：fork `deepseek-ai/deepseek-harness` → `laochenfei233/deepseek-harness`，Tauri 壳作为新目录 `apps/desktop/` 并入 fork。上游同步 = 单向 `git pull upstream`（**绝不 push 上游**）；四 preset 由上游 `apps/cli/config/agent-presets/` 提供，壳不修改任何上游 `packages/` 代码，因此同步即保留全部功能。

## [S2] Design

### 2.1 总体架构

桌面壳 = Tauri 2 应用，与社区先例同构（`xiincs/deepseek-harness-desktop`、`hairyf/deepseek-harness-desktop`、`dsh-tauri-desk/dsh-tauri` 协议）：

```
apps/desktop/
  src-tauri/            # Rust：主窗口 WebView + 子进程管理 + 托盘 + 系统集成
    src/
      main.rs           # Tauri 入口
      dsh_runner.rs     # dsh 子进程生命周期：启动/健康检查/崩溃重启/退出
      tray.rs           # 托盘常驻（关窗不退出，托盘菜单：显示/退出）
      host_rpc.rs       # POST /api/host.openPath 等宿主 RPC（session-context-menu 依赖）
      setup.rs          # 首次启动向导编排（profile 预设 + 插件安装）
      updater.rs        # 版本检查（GitHub Releases / Tauri updater）
    tauri.conf.json
    capabilities/
    icons/
  ui/                   # 壳前端：顶部导航栏（侧边栏/后退/前进）+ 首次启动向导页面
  scripts/
    bundle-runtime.mjs  # 构建「Node runtime + 裁剪 dsh 运行环境」→ resources/
    install-plugins.mjs # 向导选择的插件安装命令封装（dsh plugin add）
```

运行模型：

1. 应用启动 → `dsh_runner` 以子进程 spawn 内置 Node runtime 执行 `dsh web --no-open`（服务绑 `127.0.0.1:3080`，与官方默认一致）；
2. 主窗口 WebView 加载 `http://127.0.0.1:3080`（loopback，用户无感知）；
3. 窗口关闭 → 隐藏到托盘，dsh 子进程继续运行；托盘「退出」才真正结束进程树；
4. dsh 子进程意外退出 → 自动重启一次并回连，仍失败则托盘气泡提示原因。

### 2.2 导航桥（对齐 dsh-tauri 插件协议）

壳内 iframe/WebView 与 dsh UI 之间走 `postMessage`，协议与 `dsh-tauri-desk/dsh-tauri` README 一致（插件与壳共用同一协议，插件加载后设置 `window.__dsh_tauri_bridge__` 让位）：

- 宿主 → iframe（`source: 'dsh-desktop'`）：`dsh://sidebar:toggle`、`dsh://page:prev`、`dsh://page:next`
- iframe → 宿主（`source: 'dsh-nav-bridge'`）：`dsh://sidebar:collapsed`、`dsh://page:firsted`、`dsh://page:lasted`

顶部导航栏三控件（侧边栏/后退/前进）由壳渲染，常驻于 dsh 应用之上。

### 2.3 宿主 RPC

`dsh-session-context-menu`（v0.2.14+）依赖宿主 RPC `POST /api/host.openPath`（目录交给系统文件管理器）。壳的 dsh 子进程前需一个同源 HTTP 反向代理层或直接在 Tauri 内实现该端点；实现方式：Tauri 侧用 `tauri-plugin-http` 或轻量本地代理把 `/api/host.*` 转发到 Rust 命令（`open::that` / `opener` crate），其余路径直通 dsh 服务。**验收**：桌面端右键「在资源管理器中打开」能打开系统文件管理器。

### 2.4 内置 Node 运行时与裁剪 dsh 运行环境（大小控制）

用户选定**内置完整运行时**（离线开箱即用，接受安装包 150–300MB 量级）。`scripts/bundle-runtime.mjs` 在 CI 中：

1. 下载对应当前平台的官方 Node 发行版（LTS，≥ v22.19）；
2. `pnpm install --frozen-lockfile` + `pnpm run build`（仓库产物）；
3. `pnpm deploy` 或等价方式**裁剪**出 web profile 运行所需的依赖树（不含 devDependencies、测试、docs、vendor 源码）；
4. 四 preset 配置文件（`apps/cli/config/agent-presets/` 的 `code/cordis/minimal/standard`）原样进入运行环境；
5. 产物打进 Tauri `resources/`（`tauri.conf.json` `bundle.resources`）。

内置 `pnpm`（与 dsh 同版本族）随运行环境分发，保证 `dsh plugin` 命令与 dsh-plugin 插件中心的安装/更新/卸载在桌面端可正常执行（更新走 profile 目录内 pnpm）。

### 2.5 首次启动向导

首次运行（无已存在 profile）展示向导页（壳前端页面，非 dsh UI）：

1. **默认预设选择**：`standard` / `minimal` / `code` / `cordis` 四选一（对应上游 agent-presets，默认 `standard`）；
2. **默认插件（自动安装，不可取消）**：`dsh-plugin`（dshplugin/dsh-plugin-hub 插件中心）、`dsh-win-terminal-inspector`（仅 Windows）；
3. **可选插件（勾选安装，全部默认不勾选）**：
   - `dsh-better-sidebar`（omdsh-dev/DSH-better-sidebar）
   - `dsh-tauri`（dsh-tauri-desk/dsh-tauri，桌面导航桥）
   - `dsh-notification`（omdsh-dev/dsh-notification）
   - `dsh-session-context-menu`（baihejiangnan/dsh-session-context-menu，仅封装端适用）
   - **IM 频道插件**：`@xmanrui/dsh-im`（企微/飞书/钉钉/微信/QQ 九渠道一站式）、`dsh-lark`（飞书）、`dsh-qqbot`（QQ）
4. 向导将选择写成 profile 的 `cordis.patch.yml` / 执行 `dsh plugin --profile web add ...`（内部调用内置 pnpm），完成后进入主界面。

插件安装命令（verbatim，来自插件仓库 README）：

| 插件 | 命令 |
|---|---|
| dsh-plugin | `dsh plugin --profile web add dsh-plugin`（桌面端改为 bundle 预装 + patch insert，见 2.9） |
| dsh-win-terminal-inspector | 复制到 `<DSH_HOME>\profiles\web\plugins\` + `cordis.patch.yml` insert（`win-terminal-inspector` 行，name `./plugins/dsh-win-terminal-inspector/index.js`） |
| dsh-better-sidebar | `dsh plugin --profile web add dsh-better-sidebar@latest`（pnpm 拦构建脚本时 `pnpm approve-builds --all` 后重跑） |
| dsh-tauri | `dsh plugin --profile web add dsh-tauri` |
| dsh-notification | `dsh plugin --profile web add https://github.com/omdsh-dev/dsh-notification/archive/refs/tags/v0.1.3.tar.gz` |
| dsh-session-context-menu | `dsh plugin --profile web add github:baihejiangnan/dsh-session-context-menu` |
| dsh-im | `dsh plugin --profile web add -w @xmanrui/dsh-im` |
| dsh-lark | `dsh plugin --profile web add dsh-lark` |
| dsh-qqbot | `dsh plugin --profile web add dsh-qqbot` |

### 2.6 四模式（preset）保留与更新功能

- 壳不修改 `packages/` 与 `apps/cli/config/agent-presets/`，四 preset 功能由上游代码提供；
- 插件更新/卸载走 dsh UI（dsh-plugin 插件中心）或 `dsh plugin` 命令，依赖内置 pnpm 与可写 profile（用户数据目录 `~/.dsh`），壳不拦截；
- 桌面端自身更新：Windows 走 Tauri updater（tauri-action 产物签名），macOS/Linux 提供「检查更新 → 打开 Releases 页」指引。

### 2.7 CI（GitHub Actions）

`.github/workflows/desktop-release.yml`：

- 触发：`workflow_dispatch` + 打 tag（`v*`）；
- matrix：`windows-latest` / `macos-latest` / `ubuntu-latest`；
- 每 job：安装 Node + pnpm + Rust → `pnpm install --frozen-lockfile` → `pnpm run build` → `scripts/bundle-runtime.mjs` 产出平台运行时 → `tauri-action` 打包（Windows `.msi/.exe`、macOS `.dmg/.app`、Linux `.deb/.AppImage`）→ 上传 GitHub Release；
- 产物附带 `THIRD_PARTY_NOTICES`。

`.github/workflows/desktop-sync.yml`：定时/手动 `git fetch upstream` + 合并 master 到本仓库（不 push 上游），合并后推送 origin。

### 2.8 目录与文件约定

- 壳代码全部在 `apps/desktop/`（新增，不触碰上游文件）；
- `apps/desktop/README.md` 说明构建与开发；
- 首次向导 UI 使用壳自带静态前端（无框架或轻框架），经 Tauri 初始窗口加载。

### 2.9 插件中心预装与内容链接外开

- **插件中心预装**：`dsh-plugin`（npm 包，对应 dshplugin/dsh-plugin-hub）由 `scripts/bundle-runtime.mjs` 在打包期以 `dsh-plugin@latest` 装进 `runtime/plugins/dsh-plugin`（`--prod --ignore-scripts`，flatten 闭包），首启时由 `setup.rs` 复制进 `$DSH_HOME/profiles/node_modules/dsh-plugin`（launcher 维护的扁平模块回退目录，裸包名 `dsh-plugin` 从 profile 经父目录上溯即可解析），并在 `cordis.patch.yml` 写 `insert` 行（`id: dsh-plugin`，name 为裸包名 `dsh-plugin`）——客户端模块注册表按条目名解析该包以提供「设置 → 插件中心」标签；旧实现的路径式 name（`./plugins/.../lib/index.js`）能加载服务端但客户端注册表解析不到，已在 `upsert_patch_row` 中迁移。
- **内容链接外开**：主窗口由 `lib.rs::build_main_window` 经 `WebviewWindowBuilder` 构建（`tauri.conf.json` 中 `create: false`），并挂 `on_new_window` 处理器：http/https/mailto/tel 通过 `tauri-plugin-opener` 交给系统默认浏览器打开，其余 scheme 一律拒绝；弹窗请求本身全部 `Deny`，应用内不产生新窗口。壳页面、向导页与 dsh iframe 内的 `target="_blank"` / `window.open` 均走此路径。

## [S3] Out of Scope

- 不修改上游 `packages/`、`apps/cli` 任何源码（含四 preset 定义）；上游同步冲突需人工处理时不改语义。
- 不做 Electron 方案；不做「浏览器 + 手动 IP 访问」的启动路径（壳内 WebView 直连 loopback 是唯一 UI 路径；`dsh web` 原 CLI 仍保留，供 CLI 用户使用）。
- 不做手机端/网页端远程访问（属 dsh-im 等插件能力，不在壳内实现）。
- 不做应用内商城 UI（插件市场由 dsh-plugin 插件中心提供）；壳只做首启向导与命令封装。
- 不在本次交付内实现 macOS/Linux 自动更新签名（Windows updater 签名依赖证书，若无证书则退回「检查更新跳转 Releases」）。

## Tasks

- [x] T1: 建立 fork 仓库与 worktree（master 同步上游，分支 feat/tauri-desktop） — acceptance: `git pull upstream` 无冲突，四 preset 文件在位（covers: S1）
- [x] T2: 编写本 spec — acceptance: 全部设计节有覆盖任务，无 TBD（covers: S1, S2）
- [x] T3: 实现 `apps/desktop/src-tauri`（WebView + dsh 子进程管理 + 托盘 + host.openPath RPC + 崩溃重启） — acceptance: `cargo build`/`tauri build` 通过，`dsh web --no-open` 由壳拉起且 WebView 加载 3080（covers: S2.1, S2.3）
- [x] T4: 实现顶部导航栏 UI 与 dsh-tauri 协议桥 — acceptance: 三控件收发 postMessage 与协议一致（covers: S2.2）
- [x] T5: 实现首次启动向导（预设四选一 + 默认/可选插件安装） — acceptance: 向导生成 profile 与插件安装命令按 2.5 表执行且可重入（covers: S2.5）
- [x] T6: 实现 `scripts/bundle-runtime.mjs`（Node runtime + 裁剪 dsh 运行环境 + 内置 pnpm） — acceptance: 产物目录可离线运行 `dsh web --no-open` 且四 preset 在位（covers: S2.4）
- [x] T7: 实现 `.github/workflows/desktop-release.yml`（三平台 matrix + tauri-action + release）与 `desktop-sync.yml` — acceptance: workflow 语法有效，dry-run 触发能走到打包步骤（covers: S2.7；depends: T3, T6）
- [ ] T8: 验证桌面端四 preset 可用与插件安装/更新路径 — acceptance: 壳内切换 preset 生成不同 agent 组合；dsh-plugin 插件中心安装/更新插件成功（covers: S2.6；depends: T3, T5, T6）
- [x] T9: 默认插件市场切换 `dshmarket` → `dsh-plugin`（dshplugin/dsh-plugin-hub） — acceptance: bundle 产物 `runtime/plugins/dsh-plugin` 含 dsh-plugin 闭包，首启复制 + patch insert 指向其 `lib/index.js`（covers: S2.5, S2.9）
- [x] T10: 主窗口（含向导与 dsh iframe 内）内容链接改为系统浏览器打开 — acceptance: `on_new_window` 拦截弹窗请求，http/https/mailto/tel 交默认浏览器且应用内不产生新窗口（covers: S2.9）
- [x] T11: 首启向导可选插件默认全部不勾选（含 dsh-tauri 导航桥） — acceptance: 向导渲染后 `dsh-tauri` 等可选插件复选框默认未选中（covers: S2.5）
