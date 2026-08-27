# Agent Note: 桌面 hub 端口弹性

Status: implemented

[English](2026-08-27-desktop-hub-port-resilience.md) | 中文

## Problem

壳与内置 `dsh web` 在三处写死端口 3081：spawn 参数、iframe 地址、CSP。无关进程占用 3081 时 hub 绑定失败并重启循环至 "dsh crashed repeatedly" 横幅；无关 HTTP 服务占据 3081 时，因就绪探测只测 TCP 连通性而被当作"已就绪"。

## Decision

端口选择移入监督循环并在每次 spawn 时执行：3081 可绑定则用 3081，否则取其上方 50 个端口区间内第一个可绑定端口。`spawn_child` 返回所选端口；监督循环将其贯穿 `wait_ready`、端口接管检查与 `dsh://ready` / `dsh://restarted-by-plugin` 事件负载。前端不再硬编码源地址：先探测 `dsh_status`（hub 已运行）否则从 ready 事件端口加载 iframe，仅纯浏览器预览保留 3081 回退。构建期 CSP 放宽为环回端口通配（`http://127.0.0.1:*`、`ws://127.0.0.1:*`），因为编译进二进制的 CSP 无法运行时更改。占用者释放后重启自然回归 3081；不持久化任何端口。

探测为 bind 探测，drop 后交给子进程绑定；微小竞态由既有的"未就绪即重启"路径吸收。不做 HTTP 探测区分占用者是 dsh 还是他人——3081 忙碌即选空闲端口，选择记录在 `desktop-startup.log`。

## Alternatives considered

**探测占用者是否为 dsh。** 否决：区分响应脆弱，双实例场景可从启动日志诊断。

**CSP 保持精确 3081 并运行时改写。** 否决：Tauri 在构建期把 CSP 嵌入二进制。

**持久化所选端口。** 否决：每次 spawn 重评估，端口空闲时自然回归 3081。

## Verification

`cargo test` 覆盖 `select_port` 空闲时返回默认端口、跳过被占端口；`cargo check`、壳页面与打包脚本的 `node --check`、vitest 插件套件全部通过。端到端流程由桌面安装验证。

## Consequences

hub 在默认端口被占时仍能启动，壳总是加载 hub 实际服务的端口。被无关服务占据的 3081 不再渲染进窗口。扫描区间限制为 50 个端口；区间耗尽时通过启动日志与错误横幅响亮失败。
