# 07 — 安全设计

## 本地监听

- SOCKS 默认 `127.0.0.1`。
- Controller 默认 `127.0.0.1`，随机高位端口，随机 secret。
- Internal provider bridge 默认 `127.0.0.1`，随机 secret/token。
- v1 不提供一键 `0.0.0.0` 公网代理。
- 若未来加 LAN 模式，必须强制认证并显示风险提示。

## Secret storage

敏感：订阅 URL token、节点 URI、用户密码、refresh token、Mihomo controller secret。

- 日志统一 redaction。
- SQLite 敏感列加密。
- Windows 使用 DPAPI/Credential Manager 保护本机 master key/token。
- 崩溃报告默认不包含订阅正文/config.yaml 全文。

## Subscription fetch

- 只允许 http/https URL。
- 防 SSRF 的云端不是问题，因为抓订阅发生在客户端；但客户端仍避免 file:// 等危险 scheme。
- Body size 上限 10 MiB。
- 连接、TLS、总超时。
- Redirect 次数上限。
- 默认校验证书；`skip TLS verify` 只做高级选项并醒目标记。

## Mihomo core update

- 固定来源和 SHA-256。
- 下载后先验 checksum 再替换。
- 原子更新，保留上一版本回滚。
- 不允许 UI 从任意 URL 下载并执行 Core。

## Cloud

- 密码 Argon2id。
- Refresh token 可撤销且数据库仅存 hash。
- Rate limit 登录接口。
- Docker 容器非 root（能做到则做）。
- /data 权限最小化。
- release 客户端要求 HTTPS，自签名仅 dev 明确允许。
