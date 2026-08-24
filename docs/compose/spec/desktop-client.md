---
feature: desktop-client
status: in-progress
updated: 2026-08-24
branch: feat/tauri-desktop
commits: # filled at delivery
---

# Desktop Client（Tauri 全平台桌面客户端）

## Report

## [S1] Problem

DeepSeek Harness（`dsh`）目前是 CLI + 浏览器形态：`dsh web` 在 `http://127.0.0.1:3080` 起本地服务并打开系统浏览器。用户需要一款**桌面独立客户端**：双击即用、关窗不退出（托盘常驻）、无需理解端口与浏览器标签页、安装包尽量小（选定 Tauri 而非 Electron）、全平台（Windows / macOS / Linux）发行，并由 GitHub Actions 打包 release 包。同时四个 agent preset（`standard` / `minimal` / `code` / `cordis`）的全部功能必须原样保留且可正常运行，插件安装/更新（dsh plugin、dsh-market）等机制在桌面端也必须可用。

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

内置 `pnpm`（与 dsh 同版本族）随运行环境分发，保证 `dsh plugin` 命令与 dsh-market 的安装/更新/卸载在桌面端可正常执行（dsh-market 的更新走 profile 目录内 pnpm）。

### 2.5 首次启动向导

首次运行（无已存在 profile）展示向导页（壳前端页面，非 dsh UI）：

1. **默认预设选择**：`standard` / `minimal` / `code` / `cordis` 四选一（对应上游 agent-presets，默认 `standard`）；
2. **默认插件（自动安装，不可取消）**：`dshmarket`（dsh-market）、`dsh-win-terminal-inspector`（仅 Windows）；
3. **可选插件（勾选安装）**：
   - `dsh-better-sidebar`（omdsh-dev/DSH-better-sidebar）
   - `dsh-tauri`（dsh-tauri-desk/dsh-tauri，桌面导航桥）
   - `dsh-notification`（omdsh-dev/dsh-notification）
   - `dsh-session-context-menu`（baihejiangnan/dsh-session-context-menu，仅封装端适用）
   - **IM 频道插件**：`@xmanrui/dsh-im`（企微/飞书/钉钉/微信/QQ 九渠道一站式）、`dsh-lark`（飞书）、`dsh-qqbot`（QQ）
4. 向导将选择写成 profile 的 `cordis.patch.yml` / 执行 `dsh plugin --profile web add ...`（内部调用内置 pnpm），完成后进入主界面。

插件安装命令（verbatim，来自插件仓库 README）：

| 插件 | 命令 |
|---|---|
| dshmarket | `dsh plugin --profile web add dshmarket` |
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
- 插件更新/卸载走 dsh UI（dsh-market）或 `dsh plugin` 命令，依赖内置 pnpm 与可写 profile（用户数据目录 `~/.dsh`），壳不拦截；
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

## [S3] Out of Scope

- 不修改上游 `packages/`、`apps/cli` 任何源码（含四 preset 定义）；上游同步冲突需人工处理时不改语义。
- 不做 Electron 方案；不做「浏览器 + 手动 IP 访问」的启动路径（壳内 WebView 直连 loopback 是唯一 UI 路径；`dsh web` 原 CLI 仍保留，供 CLI 用户使用）。
- 不做手机端/网页端远程访问（属 dsh-im 等插件能力，不在壳内实现）。
- 不做应用内商城 UI（插件市场由 dsh-market 提供）；壳只做首启向导与命令封装。
- 不在本次交付内实现 macOS/Linux 自动更新签名（Windows updater 签名依赖证书，若无证书则退回「检查更新跳转 Releases」）。

## Tasks

- [ ] T1: 建立 fork 仓库与 worktree（master 同步上游，分支 feat/tauri-desktop） — acceptance: `git pull upstream` 无冲突，四 preset 文件在位（covers: S1）
- [ ] T2: 编写本 spec — acceptance: 全部设计节有覆盖任务，无 TBD（covers: S1, S2）
- [ ] T3: 实现 `apps/desktop/src-tauri`（WebView + dsh 子进程管理 + 托盘 + host.openPath RPC + 崩溃重启） — acceptance: `cargo build`/`tauri build` 通过，`dsh web --no-open` 由壳拉起且 WebView 加载 3080（covers: S2.1, S2.3）
- [ ] T4: 实现顶部导航栏 UI 与 dsh-tauri 协议桥 — acceptance: 三控件收发 postMessage 与协议一致（covers: S2.2）
- [ ] T5: 实现首次启动向导（预设四选一 + 默认/可选插件安装） — acceptance: 向导生成 profile 与插件安装命令按 2.5 表执行且可重入（covers: S2.5）
- [ ] T6: 实现 `scripts/bundle-runtime.mjs`（Node runtime + 裁剪 dsh 运行环境 + 内置 pnpm） — acceptance: 产物目录可离线运行 `dsh web --no-open` 且四 preset 在位（covers: S2.4）
- [ ] T7: 实现 `.github/workflows/desktop-release.yml`（三平台 matrix + tauri-action + release）与 `desktop-sync.yml` — acceptance: workflow 语法有效，dry-run 触发能走到打包步骤（covers: S2.7；depends: T3, T6）
- [ ] T8: 验证桌面端四 preset 可用与插件安装/更新路径 — acceptance: 壳内切换 preset 生成不同 agent 组合；dsh-market 安装/更新插件成功（covers: S2.6；depends: T3, T5, T6）
