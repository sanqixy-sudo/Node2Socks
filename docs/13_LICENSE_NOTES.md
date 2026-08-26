# 13 — 开源参考与许可证注意事项

此文件不是法律意见。发布前请保留第三方组件的许可证和来源说明，避免复制受限代码或视觉资源。

## Clash Verge Rev

Clash Verge Rev 以 GPL-3.0 发布。本项目仅参考其公开的桌面生命周期、托盘和打包行为，不复制其源码。

## Mihomo

Mihomo 以 GPL-3.0-or-later 发布。Node2Socks 将 Mihomo 作为独立 sidecar 可执行文件，通过配置文件和 localhost HTTP API 交互，不链接其 Go 包。分发时保留 `sidecar/LICENSES/` 下的 notice 和许可证文本，并按 pinned 版本校验二进制。

## Node2Socks

Node2Socks 自身授权范围见仓库根目录 `LICENSE`。第三方依赖仍按各自许可证执行。