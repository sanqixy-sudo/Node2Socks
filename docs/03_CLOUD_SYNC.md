# 03 — 自建 Docker 云同步

## 1. 用户体验

Node2Socks 默认本地模式。云同步页面允许：

```text
服务器地址
https://sync.example.com

[检测服务器]

账号
密码
[登录] [注册]
```

客户端不内置官方固定云地址。用户可以部署自己的任何域名。

## 2. 云端部署形态

v1 目标：**单 Docker 容器 + 单持久卷**。

```text
Internet
   ↓ HTTPS
用户自己的 Nginx/Caddy/Cloudflare
   ↓ HTTP 127.0.0.1/内网:8080
node2socks-cloud Docker
   ↓
/data/node2socks-cloud.db  (SQLite WAL)
```

云容器不申请证书，不强绑定域名。TLS 和反代由用户处理。

## 3. Docker 约束

- Image 暴露 `8080`。
- `/data` 持久化。
- `restart: unless-stopped`。
- 健康检查 `/healthz`。
- 数据库 migrations 启动时自动执行。
- 日志 stdout/stderr。
- `TRUST_PROXY=true` 时只信任标准反代头并支持允许的 proxy CIDR（后续）。

v1 SQLite 足够用于个人/小团队自建。Repository 层不要写死 SQLite 专属 SQL，后续可增 PostgreSQL。

## 4. 云端绝不做的事

- 不接收 SOCKS5 流量。
- 不连接机场节点。
- 不替客户端刷新订阅。
- 不保存实时浏览记录。
- 不作为远程代理网关。

因此云服务宕机不会影响已运行的本地 Slot。

## 5. 同步内容

同步：

- subscriptions：名称、URL、刷新周期、headers、启用状态、最后成功缓存。
- nodes catalog：显示元数据、稳定 ID、来源、最后出现时间。
- proxy_slots：slot_id、端口、名称、备注。
- bindings：slot -> node。
- tags/groups。
- 用户设置（不含设备局部设置）。

设备局部不同步：

- 当前网卡名称。
- Mihomo controller port/secret。
- 本地日志目录。
- 当前延迟/连接数/速率。
- 当前端口占用进程。

## 6. Offline-first

所有变更先写本地 DB，再写 `sync_outbox`。同步 Worker 后台上传。

云不可用：

```text
本地修改成功
↓
outbox pending
↓
本地代理照常
↓
网络恢复
↓
自动上传
```

不能因为登录过期停止本地 Core。

## 7. 加密

订阅 URL、URI、节点 token/UUID/password 都是敏感数据。

v1 设计：

- 本地 DB 中的敏感 payload 用随机 `vault_key` 做 AEAD 加密（AES-256-GCM 或 ChaCha20-Poly1305）。
- Windows 本地通过 DPAPI/Credential Manager 保护本机 key material。
- 云端保存的是客户端加密后的 ciphertext record。
- 新设备登录后，通过账号密码派生的 KEK 解包云端保存的 wrapped vault key，实现“重装后只登录即可恢复”。
- 服务端账号密码使用 Argon2id hash。
- 由于服务端在标准 HTTPS 登录中会接触密码，这不是抵抗恶意服务器的严格 zero-knowledge PAKE。文档中称“客户端加密/云端密文存储”，不要虚假宣传零知识。

后续如果需要真正零知识，可增加独立 recovery secret/OPAQUE，但不作为 v1 阻塞项。

## 8. 设备

每次登录创建/复用 `device_id`：

- 设备名称
- OS
- app version
- first_seen
- last_seen
- revoked_at

用户可以注销其他设备。被 revoke 的 refresh token 失效，但该设备本地模式仍可用。

## 9. 首次启用云同步

若本地已有数据且云端也有数据，必须明确给用户：

1. 合并（默认推荐）。
2. 用本地覆盖云端。
3. 用云端覆盖本地。

任何“覆盖”需要二次确认并先创建本地自动备份。
