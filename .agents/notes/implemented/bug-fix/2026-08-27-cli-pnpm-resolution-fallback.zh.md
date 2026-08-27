# Agent Note: CLI pnpm 解析回退

Status: implemented

[English](2026-08-27-cli-pnpm-resolution-fallback.md) | 中文

## Problem

调用 shell 缺少 pnpm 时，`dsh plugin --profile web add dsh-plugin` 报 `pnpm not found on PATH` 并以 127 退出。桌面包虽内置 pnpm@11.7.0（bundle-runtime 把包装入 `runtime/node/node_modules/pnpm`）但从未生成可执行 shim，CLI 也只在 PATH 上解析 pnpm 且无回退。

## Decision

两部分补上缺口。

**内置 pnpm shim。** bundle-runtime 在包复制后向 `runtime/node/node_modules/.bin` 写 `pnpm`/`pnpx` shim：POSIX shim 相对自身解析 node 二进制（installNode 扁平化后的 `../../node`），`.cmd` shim 用 `%~dp0\..\..\node.exe`。相对解析保证运行时在构建机与安装包之间可迁移。壳已将该 `.bin` 目录前置到 spawn 的 `dsh web` PATH，市场同样能找到 pnpm。

**CLI 解析 pnpm。** `resolvePnpm()` 依序探测候选——`PNPM_BINARY` 覆盖、PATH 上的 `pnpm`、以及由本 CLI `INSTALL_ANCHOR` 推导的运行时同级 shim `<runtime>/node/node_modules/.bin/pnpm`（从 `runtime/dsh/package.json` 上溯两级）——接受第一个 `--version` 探测退出 0 的候选。全部失败时打印 `npm install -g pnpm` 或 `corepack enable pnpm` 引导后返回 127。`$DSH_HOME/node_modules` junction 不作候选：它指向 deploy 闭包的 node_modules，其中没有 pnpm。shell 模式探测把旗标拼入命令串（规避 DEP0190 警告）并对含空格路径加引号。

## Alternatives considered

**同时向闭包 `.bin` 写 shim。** 否决：闭包没有 pnpm 包可供相对 shim 调用，向 deploy 闭包复制 pnpm 只是重复运行时副本而无可达性收益。

**自动 `corepack pnpm` 回退。** 否决：strict 模式 corepack 需要 `packageManager` 字段且可能抓取未固定版本的 pnpm，与运行时固定的 11.7.0 冲突。

## Verification

vitest 套件覆盖 `PNPM_BINARY` 优先级、全缺失返回 undefined、`runtimePnpmCandidate` 解析 deploy 布局并对 npm 全局布局返回 undefined。`tsc --noEmit` 与打包脚本的 `node --check` 通过。

## Consequences

桌面 runtime 的 CLI 无需全局 pnpm 即可安装插件，且与市场使用同一固定版本。无 pnpm 的 npm 全局安装仍以 127 退出，但附带安装引导。
