<div align="center">

<img src="apps/desktop/public/node2socks-logo.png" width="96" alt="Node2Socks logo">

# Node2Socks

**轻量、独立、仅本机监听的 Windows SOCKS5 代理工作台**

管理订阅与节点，为每个代理 Slot 分配稳定端口；Node2Socks 自己运行 Mihomo，不依赖用户的 Clash 进程。

[![CI](https://github.com/sanqixy-sudo/Node2Socks/actions/workflows/ci.yml/badge.svg)](https://github.com/sanqixy-sudo/Node2Socks/actions/workflows/ci.yml)
[![Source available](https://img.shields.io/badge/source-public-blue.svg)](LICENSE)
[![Mihomo](https://img.shields.io/badge/Mihomo-v1.19.30-1683dc.svg)](sidecar/LICENSES/MIHOMO_NOTICE.md)

</div>

## 项目简介

Node2Socks 面向需要多个独立 SOCKS5 出口的个人和小团队。它把订阅或手动节点解析为可管理的节点列表，并将节点绑定到稳定的 Proxy Slot：

- 默认监听 `127.0.0.1`，不会把代理端口暴露到局域网。
- 一个 Slot 对应一个固定端口；更换节点不会改端口。
- 已绑定节点消失时自动 fail-closed，不会静默换成其他节点。
- 内置独立 Mihomo Core、延迟测试、出口检测、托盘后台和可选的加密云同步。
- 关闭窗口进入托盘；从托盘菜单选择“退出”才会真正停止进程。

> 当前版本是未签名的工程候选版。环境相关验收项请参阅 [验收文档](docs/09_TEST_AND_ACCEPTANCE.md) 和 [实现状态](docs/14_IMPLEMENTATION_STATUS.md)。

## 功能概览

| 模块 | 能力 |
| --- | --- |
| 订阅 | Clash/Mihomo YAML、URI 列表与 Base64；刷新周期、请求头、自定义下载代理、错误保留 |
| 节点 | 按订阅分组、搜索/筛选、Google 204 延迟测试（5 秒超时、并发 6）、出口信息 |
| Proxy Slot | 稳定端口、节点重绑、端口冲突提示、批量创建/删除、复制带备注的 SOCKS5 地址 |
| Core | 独立 Mihomo sidecar、崩溃恢复、配置校验、仅 localhost Controller |
| 托盘 | 显示/隐藏窗口、启动/停止 Core、复制代理、打开数据目录、真正退出 |
| 云同步（可选） | 自托管 Axum + SQLite WAL；只同步加密状态，不承载代理流量 |

## 安全边界

- 代理 Slot、Provider Bridge、Mihomo Controller 默认只绑定 `127.0.0.1`。
- 订阅下载使用 `no_proxy()`，不会隐式继承 Windows 系统代理。
- 订阅 URL、请求头、密码和令牌不会写入应用日志；本地敏感字段使用 DPAPI/加密信封保护。
- 云服务是可选的状态同步服务，不转发用户代理流量，也不能替代本地 Core。

## 快速开始（Windows 10/11 x64）

### 准备环境

安装 Node.js 20+、pnpm 11、Rust stable、Microsoft C++ Build Tools 和 WebView2。然后：

~~~powershell
pnpm install --frozen-lockfile
.\scripts\prepare_sidecar.ps1
~~~

脚本从 Mihomo 官方 v1.19.30 Release 下载 Windows amd64 包并校验 SHA-256。下载的二进制不会被 Git 跟踪。

### 开发运行

~~~powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
pnpm --dir apps/desktop tauri dev
~~~

### 质量检查

~~~powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm --dir apps/desktop test
pnpm --dir apps/desktop check
pnpm --dir apps/desktop build
~~~

### Windows 构建

~~~powershell
pnpm --dir apps/desktop tauri build
.\scripts\build_portable.ps1
~~~

便携版存在 `portable.flag` 时，数据写入解压目录的 `data/`。

## 可选云服务

云服务不参与代理流量，只同步端到端加密的应用状态：

~~~powershell
Copy-Item examples/cloud/.env.example examples/cloud/.env
# 编辑 .env，设置至少 32 字符的 NODE2SOCKS_CLOUD_JWT_SECRET
docker compose --env-file examples/cloud/.env -f examples/cloud/docker-compose.yml up -d --build
~~~

生产部署请使用 HTTPS 反向代理；参阅 [Cloud 部署说明](docs/12_DEPLOYMENT.md) 和 [Nginx 示例](examples/cloud/nginx.example.conf)。

## 仓库结构

- `apps/desktop`：React + TypeScript UI 与 Tauri 2 壳层。
- `crates/core-adapter`：ProxyCore 边界与 Mihomo sidecar 生命周期。
- `crates/storage`：SQLite 迁移和本地持久化。
- `services/cloud`：可选 Axum/SQLite WAL 同步服务。
- `migrations`：本地与云端数据库迁移。
- `fixtures`：只含 `.invalid`/文档地址的测试输入，不代表可用订阅。
- `docs`：架构、安全、部署、验收与 UI 说明。

## 开源与第三方许可

Node2Socks 自身源代码为公开可审阅但保留权利，详见 [LICENSE](LICENSE)。Mihomo 是独立 sidecar，按 GPL-3.0-or-later 发布；分发时请保留 [Mihomo notice](sidecar/LICENSES/MIHOMO_NOTICE.md) 和许可证文本。Clash Verge Rev 与其他参考资料仅用于行为/架构研究，不复制其源码或视觉资源。

未经许可证核验的用户上传参考快照不会纳入公开 Git 文件；相关本地目录已被 `.gitignore` 排除。

## 贡献与反馈

欢迎提交 Issue/PR。请不要在公开 Issue、日志或截图中粘贴订阅令牌、代理账号密码、内网地址或个人数据。