# Clash Verge Rev reference snapshot

Repository: https://github.com/clash-verge-rev/clash-verge-rev
Branch observed: `dev`
License: GPL-3.0

公开 README 明确其基于 Rust + Tauri 2，并内置 Mihomo。当前 `src-tauri/tauri.conf.json` 使用 Tauri `externalBin` 管理 `sidecar/verge-mihomo` 与 alpha core。这正是 Node2Socks 要参考的 sidecar 包装模式。

关键路径：

```text
src-tauri/tauri.conf.json
src-tauri/src/core/
src-tauri/src/core/manager/
src-tauri/src/core/autostart.rs
src-tauri/src/core/runtime_bundle.rs
src-tauri/src/core/service.rs
src-tauri/src/core/updater.rs
```

请运行本包 `scripts/fetch_upstream_references.ps1` 获取完整仓库快照供 Codex 搜索。不要直接复制 GPL 代码。
