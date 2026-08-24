# dsh-desktop — DeepSeek Harness 桌面客户端

Tauri 2 桌面壳：在原生窗口里内嵌 dsh web UI，内置 Node 运行时与裁剪后的 dsh 发行，开箱即用、关窗不退出（托盘常驻）、服务崩溃自动重启。**不修改任何上游 `packages/` / `apps/cli` 代码**，四个 agent preset（`standard` / `code` / `minimal` / `cordis`）由上游原样提供。

## 架构

```
src-tauri/            Rust 壳
  src/dsh_runner.rs   dsh 子进程生命周期：spawn `dsh web --port 3081 --no-open`、端口健康探测、崩溃自动重启（dsh-market 自行重启时不竞争）
  src/tray.rs         托盘常驻：关窗隐藏，托盘「退出」才结束进程树
  src/setup.rs        首次启动向导命令：预设写入 profile patch（agent-presets.config.default）、插件安装（dsh plugin / 本地复制）
ui/dist/              壳前端（纯静态，无构建）：index.html 导航栏 + iframe；wizard.html 首次向导
scripts/bundle-runtime.mjs  打包 Node 运行时 + pnpm deploy 的 dsh 闭包 + 本地插件 → runtime/
runtime/              打包产物（不入库，CI 生成；tauri.conf.json 将其映射为 bundle resources）
```

端口约定：壳内 `dsh web` 固定监听 `127.0.0.1:3081`，WebView 同源直连（CLI 用户的 `dsh web` 默认 3080 不受影响）。`/api`（含 `host.openPath`、WebSocket 下行、SSE）全部由 dsh 自身提供，壳不做反向代理。导航栏与 dsh UI 之间走 `dsh-tauri` 插件定义的 postMessage 协议（`dsh://sidebar:toggle` 等，`source: 'dsh-desktop'` / `'dsh-nav-bridge'`）。

## 开发

前置：Node ≥ 22.19、pnpm ≥ 11、Rust ≥ 1.77、Tauri CLI（随本包 devDependency 提供）。

```sh
pnpm install                # 仓库 workspace 依赖
pnpm run build              # 生成 lib 与 apps/web/dist（dsh web 的前端静态文件）
pnpm tauri dev              # 从 apps/desktop 运行；dev 模式用 PATH 上的 node + apps/cli/lib/bin.js
```

dev 模式需要已构建的 CLI 产物（`pnpm run build`）；也可用环境变量覆盖运行时解析：
`DSH_DESKTOP_NODE`（node 可执行）、`DSH_DESKTOP_DSH_BIN`（dsh CLI 入口）、`DSH_DESKTOP_PLUGINS_DIR`（本地插件目录）。

## 构建发行版

```sh
pnpm run build              # 仓库产物
node scripts/bundle-runtime.mjs --platform win   # win|mac|linux；生成 runtime/
pnpm tauri build            # 打包安装包（runtime/ 随 resources 进入）
```

CI（`.github/workflows/desktop-release.yml`）在三平台矩阵上执行同一流程并上传 GitHub Release；打 `v*` tag 或手动触发。

## 首次启动

- 默认预设：`standard`（可改 `code` / `minimal` / `cordis`，写入 `~/.dsh/profiles/web/cordis.patch.yml` 的 `agent-presets.config.default`）
- 默认插件：`dshmarket`（插件市场）、`dsh-win-terminal-inspector`（仅 Windows，本地复制 + patch insert）
- 可选插件：`dsh-better-sidebar`、`dsh-tauri`（导航桥）、`dsh-notification`、`dsh-session-context-menu`、`@xmanrui/dsh-im`（企微/飞书/钉钉/微信/QQ 等 9 渠道）、`dsh-lark`、`dsh-qqbot`

插件安装/更新/卸载全部走 dsh 官方机制（`dsh plugin` / dsh-market），运行时内置 pnpm 与其同版本族。
