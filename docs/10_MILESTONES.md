# 10 — 开发里程碑

## M0 — Repo scaffold（0.5–1 天）

- Rust workspace + Tauri 2 + React。
- Cloud service crate/app。
- Shared domain crate。
- SQLite migration framework。
- CI：format/lint/test。

完成条件：空壳 UI + Rust command + SQLite migration + cloud `/healthz`。

## M1 — Mihomo lifecycle POC（1–2 天）

- 固定 Mihomo v1.19.30 Windows amd64 sidecar。
- Core manager start/stop/restart/log capture/health。
- localhost Controller secret。
- 手工测试一个 SOCKS listener。

完成条件：Core POC 可重复启动 20 次无残留进程/端口。

## M2 — Proxy Slot POC（2–3 天）

- Slot DB。
- 2+ listeners。
- selector 切换 API。
- 端口分配与冲突检测。
- 绑定持久化。

完成条件：两端口两节点不同出口，重启保持。

## M3 — Subscription + Provider Bridge（3–5 天）

- fetch + format detect/cache。
- local provider HTTP bridge。
- Mihomo providers + prefix uniqueness。
- node catalog via Controller API。
- manual/auto refresh。

完成条件：至少 Clash YAML、URI、Base64 三类订阅可加载。

## M4 — Fail-closed & Health（2–3 天）

- bound node removed test。
- BLOCK fallback。
- delay + exit IP test。
- orphaned UI state。

这是 release blocker。

## M5 — 正式 UI（3–5 天）

首页、订阅、节点、Slot、设置、诊断、托盘、自启动。

## M6 — Clash coexistence（2–4 天）

- detection。
- outbound interface selection。
- TUN real-device matrix。
- diagnostics。

## M7 — Cloud Docker（4–6 天）

- register/login/token/device。
- encrypted sync records。
- outbox/offline sync/conflict。
- Dockerfile/Compose/health。
- custom base URL UI。

## M8 — Recovery & Packaging（2–4 天）

- backup/restore。
- fresh-install cloud recovery。
- installer/updater strategy。
- license notices / core checksum。
- final acceptance suite。

## M9 — Optional v1.1

- AdsPower Local API integration。
- automatic Slot -> AdsPower proxy import。
- LAN authenticated listeners。
- macOS support。
- PostgreSQL cloud backend。
