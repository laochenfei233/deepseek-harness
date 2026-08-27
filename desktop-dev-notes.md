# DeepSeek Harness 桌面端（apps/desktop）开发笔记

> 本文档汇总桌面客户端（Tauri 2 壳）一次完整迭代的实现内容、关键机制、注意事项与排错记录。
> 覆盖提交范围：`57da7798a..HEAD`（master）。代码以当前 master 为准，本文档为上下文快照。

## 一、已实现功能与改动

### 1. 内置插件市场切换为 dsh-plugin（dshplugin/dsh-plugin-hub）
- **产物**：`scripts/bundle-runtime.mjs` 打包期把 `dsh-plugin@latest` 装进 `runtime/plugins/dsh-plugin`（`--prod --ignore-scripts`，flatten 闭包；`pnpm-workspace.yaml` 置 `minimumReleaseAge: 0` 防止 pnpm 11 跳过最新发行）。
- **首启预装**：`setup.rs::ensure_default_plugins` 复制进 `$DSH_HOME/profiles/node_modules/dsh-plugin`（launcher 维护的扁平模块回退目录），并写 `cordis.patch.yml` insert 行（`id: dsh-plugin`，**name 用裸包名** `dsh-plugin`）。
- **提交**：`e593c583e`、`c9f6251fa`。

### 2. 内容链接左键直开默认浏览器
- **机制**：宿主桥（见「关键机制」）在 dsh 前端注入 click 拦截——`target="_blank"` 或外域 http(s)/mailto 链接左键 → postMessage 给壳 → `plugin:opener|open_url` → 默认浏览器。
- **保留**：Rust 侧 `on_new_window` 处理器（右键菜单"新标签页打开"等 NewWindowRequested 路径）+ `cmd /c start` 兜底 + `new-window.log` 诊断日志。
- **提交**：`c62acf563`、`41ba9832b`。

### 3. 通知（原生 Windows toast）
- **机制**：宿主桥在 iframe 内把 `Notification` 替换为 postMessage 桥（permission 恒为 granted），壳收到后调 `tauri-plugin-notification` 弹原生通知。
- **提交**：`13a6f42b6`。

### 4. 插件安装不再弹 cmd 窗口
- **根因**：Node `windowsHide` 默认 **false**，GUI 壳（无控制台）派生的 console 子进程各自新建可见窗口。
- **修复**：桌面壳启动时 `AllocConsole()` + `SW_HIDE`（后代全部挂到隐藏控制台）+ dsh CLI pnpm spawn 显式 `windowsHide: true`。
- **提交**：`ffa0e31a2`。

### 5. 顶部导航栏彻底移除
- 壳前端去掉导航栏（侧边栏/后退/前进/状态点），iframe 占满窗口；错误横幅保留兜底。`dsh-tauri` 导航桥插件从向导移除。
- **提交**：`016396664`。

### 6. 首启向导
- 可选插件列表：`dsh-better-sidebar`、`dsh-notification`、`dsh-session-context-menu`、`@xmanrui/dsh-im`、`dsh-lark`、`dsh-qqbot`、`dsh-vision-router`、`graph-memory`（`github:adoresever/graph-memory`）、`aegis`（`github:ganyuanran/aegis`）；全部默认不勾选。
- **向导显示修复**：`ensure_default_plugins` 预写 patch 会让初始化判定误判为"已初始化"→ 正式安装版向导永不出现。改为以壳自己的 `desktop.json`（仅向导完成时写入）判定。
- **完成后重启 dsh**：让向导勾选的插件 bundle 立即生效。
- **提交**：`3922e17af`、`89e3834fa`、`269f2606a`、`4176bbd27`、`41ba9832b`。

### 7. MCP 与 Skill 默认支持
- **Skill**：已在 base bundle（`@deepseek-ai/dsh-skill` 等 4 包）默认启用。
- **MCP**：`@deepseek-ai/dsh-mcp-client` 在运行时闭包但未组合；首启时向 `cordis.patch.yml` 追加注释模板（serverName/transport/command 示例），取消注释即接入本地 MCP 服务器并热重载。
- **已实测**：本地 stdio MCP 服务器握手成功（initialize + tools/list），工具以 `mcp__<server>__<tool>` 注册。
- **提交**：`4176bbd27`。

### 8. macOS / Windows 双架构（每架构独立包）
- **矩阵**：`windows-latest(x64)`、`windows-11-arm(arm64)`、`macos-latest(arm64)`、`macos-latest(x64, 跨架构)`、`ubuntu-latest(x64)`。
- **bundle-runtime `--arch` 参数**：mac 默认 arm64，可显式 x64；x64 闭包用内置 x64 node 驱动 pnpm（PATH 前置 `runtime/node/bin`），原生依赖按目标架构解析。
- **Windows arm64 必须用 `windows-11-arm` 原生运行器**（x64 主机无法运行 arm64 node）。
- **闭包架构 CI 校验**：bundle 后 `file` 检查 node 与全部 `.node` 模块架构，不符即失败。
- **提交**：`2f97694c8`、`18567f093`、`eee8a7558`、`2c341a63d`。

### 9. macOS 图标缩小（不影响 Windows）
- **根因**（视觉+像素级分析）：源图鲸鱼留白充足（占画布 56%），但白色底板占 92%，macOS squircle 遮罩把底板推到视觉边缘（深色壁纸上尤甚）。
- **修复**：仅重做 `icon.icns`（底板+鲸鱼缩至 82% 居中、透明边距），Windows 的 `icon.ico` 与全部 png 不动。
- **提交**：`3fbc11751`。

### 10. CI 构建产物下载
- `upload-artifact` 按平台+架构上传（`dsh-desktop-<平台>-<架构>`），job 权限补 `actions: read`。
- **提交**：`b8071de62`。

### 11. 版本与 CI 基础修复
- 版本对齐 dsh 家族 `0.1.1-rc.2`（package.json / tauri.conf.json / Cargo.toml / Cargo.lock）。
- CI 修复：pnpm setup 版本冲突、`contents: write` 权限、POSIX Node 布局扁平化、AppImage（`APPIMAGE_EXTRACT_AND_RUN` + `libfuse2`/`squashfs-tools` + musl/异架构闭包裁剪）、`--verbose` 暴露 linuxdeploy 报错。
- **提交**：`78c57d562`、`de7e7c1c0`、`155c4cfe7`、`4c8ba8b78`、`a9a3fefd6`、`35b7a30cb`、`58038242b`、`261cfd3fc`、`4b0d4c27e`、`46adf301b`、`9176ccb11`、`da4b0fce0`。

## 二、关键机制

### 宿主桥（bundle 期注入 dsh 前端）
`bundle-runtime.mjs::injectNotificationBridge` 在 `deployDsh` **最后**（`flattenClosure`/patch 之后）把脚本注入 `runtime/dsh/node_modules/@deepseek-ai/dsh-web-frontend/dist/index.html`：
- 仅在 iframe 内激活（`window !== window.parent` 守卫；纯浏览器 `dsh web` 不受影响）。
- `Notification` → postMessage（source `dsh-desktop-host`, kind `notify`）→ 壳 `tauri-plugin-notification`。
- click 拦截 `target="_blank"`/外域链接 + `window.open` → postMessage（kind `open`）→ 壳 `plugin:opener|open_url`。
- **注入必须在 flattenClosure 之后**：flatten 会用 .pnpm store 覆盖 dist，提前注入会被冲掉。

### 插件中心客户端解析
客户端模块注册表（`packages/client/modules`）按 loader 条目名 `require.resolve('<name>/package.json')` 从 profile 解析。patch 行 `name` 必须用**裸包名**（路径形式会导致客户端入口缺失）；包本体放进 `$DSH_HOME/profiles/node_modules` 扁平回退目录即可被解析（"bundles 来自安装"契约，pnpm 不裁剪该目录）。

### 向导初始化判定
`setup.rs::read_state` 的 `initialized` 只看 `~/.dsh/desktop.json`（`set_preset` 写入）。`cordis.patch.yml`/`package.json` 的存在不代表向导已完成（它们会被 `ensure_default_plugins` 与 dsh 进程预生成）。

### 跨架构闭包
pnpm 的 os/cpu/libc 过滤基于**运行它的 node 的架构**。mac x64 构建：内置 x64 node 的 `bin/` 目录前置到 PATH（corepack pnpm shim 经 `#!/usr/bin/env node` 解析到 x64 node）。**依赖此机制是否真正生效，已由 CI 的"Verify closure architecture"步骤兜底**。

## 三、注意事项

1. **推送 GitHub 需代理**：github.com:443 间歇不可达（DNS 正常、api.github.com 正常），用 sing-box `127.0.0.1:32808`（HTTP/SOCKS5 混合端口）。推荐一次性参数：`git -c http.proxy=http://127.0.0.1:32808 push`，不改持久配置。
2. **bundle-runtime 会弄脏仓库 node_modules**：其 pnpm deploy 等操作后，`pnpm run` 会触发 pnpm 11 deps 预检且因无 TTY 报 `ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY`（pre-push 钩子也受影响）。修复：`pnpm install --config.confirmModulesPurge=false` 后即可推送。
3. **CREATE_NO_WINDOW 不向后代传播**：每个 console 子进程需各自隐藏（Node 侧用 `windowsHide: true`）。
4. **WebView2 对跨源 iframe 的 `target="_blank"` 左键不触发 NewWindowRequested**（右键"新标签页打开"才会）；WebView2 默认拒绝通知权限（wry 只处理剪贴板 PermissionRequested）。两者都需宿主桥在页面层解决。
5. **图标缓存**：重装后任务栏显示旧 logo 是 Windows 图标缓存，重启 Explorer 刷新。
6. **Gitea 仓库是私有只读镜像**（从 GitHub 拉取，`mirror repository is read-only`），不能直接 push；构建产物（dmg/exe）不在 Gitea 上（是 GitHub Actions Artifacts）。
7. **`windows-11-arm` 是 GitHub 原生 arm64 Windows 运行器**；x64 主机无法运行 arm64 node，因此 Windows arm64 闭包必须在原生 arm 运行器上构建。
8. 向导可选插件默认全部不勾选；`dsh-tauri` 已随导航栏移除而从向导删除。

## 四、常见报错速查

| 报错 | 根因 | 解决方案 |
|---|---|---|
| `Release (dsh) release:verify: dsh release members must share one version` | apps/desktop 版本 ≠ 家族 0.1.1-rc.2 | 三处版本同步（package.json / tauri.conf.json / Cargo.toml + lock） |
| `Multiple versions of pnpm specified` | pnpm/action-setup `version` 与 packageManager 冲突 | 移除 action 的 `version` 字段 |
| `Resource not accessible by integration`（tauri-action 上传 Release） | GITHUB_TOKEN 只读 | workflow 加 `permissions: contents: write` |
| `bundled npm missing at .../node_modules/npm/...`（mac/linux） | POSIX tar.xz 布局为 `bin/node` + `lib/node_modules/npm` | installNode 扁平化布局 |
| `failed to run linuxdeploy`（ubuntu AppImage） | ① 无 FUSE ② 缺 libfuse2/squashfs-tools ③ 闭包含 musl/异架构 ELF（koffi `musl_x64/`） | `APPIMAGE_EXTRACT_AND_RUN=1` + apt 补装 + 闭包裁剪（含包内 musl 子目录）+ `--verbose` 定位 |
| 设置里没有"插件中心"标签 | patch 行 name 用路径 → 客户端注册表按名解析失败 | 裸包名 + `profiles/node_modules` 回退复制 |
| 通知出不来 / 设置显示"已拒绝" | WebView2 拒绝通知权限 | 宿主桥 polyfill + tauri-plugin-notification |
| 内容链接左键无反应（右键可开） | iframe 内 target=_blank 左键不触发 NewWindowRequested | 宿主桥 click 拦截 |
| 安装插件弹 cmd 窗口 | windowsHide 默认 false + GUI 壳无控制台 | AllocConsole+SW_HIDE + pnpm windowsHide |
| 正式安装版首启向导不出现 | ensure_default_plugins 预写 patch 误判已初始化 | 判定改用 desktop.json |
| `Bad CPU type in executable (os error 86)`（Intel Mac 装插件） | mac x64 包里是 arm64 node（`--arch x64` 未被解析） | 参数解析支持 `--flag value` 空格形式 |
| `dsh exited during startup`（Intel Mac） | 疑似闭包 native 模块仍为 arm64（x64 node 加载 arm64 .node 崩溃） | CI "Verify closure architecture" 步骤定位；若命中需让 pnpm 显式跑在 x64 node 下 |
| `dsh crashed repeatedly` + hub.log 每 4 秒重复 "Plugin Hub 已启动"（macOS） | `ensure_default_plugins` 无条件插入 win-terminal-inspector 的 patch 行，而 mac/linux 包不带该插件 → loader 解析不到 → 每次启动即崩 → 看门狗循环重启 | 行插入按运行时是否含该插件门控 + 移除残留行自愈（`a43d73d3b`） |
| `directory picker failed: spawn osascript ENOENT`（macOS，同类含 npx/git 全 ENOENT） | dsh 子进程 PATH 用 `;` 拼接——Windows 正确但 mac/linux 分隔符应为 `:`，坏 PATH 使所有按 PATH 查找的子进程 spawn 失败 | 改用 `std::env::join_paths` 按平台取分隔符（`1dbb66584`） |
| `ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY` | pnpm 11 deps 预检需重建 node_modules | `pnpm install --config.confirmModulesPurge=false` |

## 五、当前状态与待办

- **CI**：架构校验 run（33033441566）构建中——验证 5 行矩阵 + 闭包架构自检；完成后确认 mac x64 与 win arm64 行的 node/原生模块均为目标架构。
- **Intel Mac "dsh exited during startup"**：待 CI 校验结果定性——若闭包 native 为 arm64，需把 bundle-runtime 的 pnpm 改为显式用内置 x64 node 调用（不再依赖 PATH 解析）。
- **Gitea**：代码已有私有只读镜像（自动从 GitHub 同步）；构建产物如需走 Gitea 分发需另找上传通道（镜像仓库只读）。
- **本地**：`apps/desktop` 有未跟踪 `.tmp-tauri-cli/`（npx 缓存的 Tauri CLI，不入库）；`~/.dsh` 保留 `sessions/`。
