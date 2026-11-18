# 仓库协作

- 桌面客户端在 `apps/desktop/`，共享 Rust 逻辑在 `crates/`，可单独部署的同步服务在 `services/cloud/`。
- Rust 工作区执行 `cargo fmt --check`、`cargo test --workspace` 和 `cargo clippy --workspace --all-targets -- -D warnings`。Windows 客户端验收前按说明准备固定版本的 Mihomo。前端用根目录锁文件安装，在 `apps/desktop/` 运行 `pnpm test`。
- 同步服务测试使用隔离 SQLite 和测试凭据。修改端口、数据卷或部署参数时核对两份 Compose。
- 同步服务发布标签为 `cloud-vX.Y.Z`，版本与 Cargo 工作区一致。Windows 客户端沿用独立的安装包发布流程。
