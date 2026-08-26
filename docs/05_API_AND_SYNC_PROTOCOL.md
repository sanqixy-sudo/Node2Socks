# 05 — 云 API 与同步协议

Base path：`/api/v1`

## 1. 非认证

- `GET /healthz`
- `GET /api/v1/server-info`
- `POST /api/v1/auth/register`
- `POST /api/v1/auth/login`

`server-info` 返回：版本、API 版本、是否允许注册、最大 payload 等。客户端输入域名后先调用它做兼容性检查。

## 2. 认证

建议 access token 15 分钟，refresh token 30 天，可撤销。

- `POST /auth/refresh`
- `POST /auth/logout`
- `POST /auth/change-password`

## 3. Vault bootstrap

- `GET /vault/bootstrap`
- `PUT /vault/bootstrap`

存：vault salt、wrapped vault key、crypto version。真正的订阅内容仍在 sync records 中以 ciphertext 保存。

## 4. Devices

- `GET /devices`
- `DELETE /devices/{id}` — revoke。

## 5. Delta sync

### Pull

`GET /sync/changes?after=<cursor>&limit=500`

返回：

```json
{
  "cursor": 12345,
  "has_more": false,
  "records": [
    {
      "type": "proxy_slot",
      "id": "...",
      "version": 4,
      "deleted": false,
      "ciphertext": "base64...",
      "nonce": "base64...",
      "updated_at": "..."
    }
  ]
}
```

### Push

`POST /sync/changes`

```json
{
  "changes": [
    {
      "type": "proxy_slot",
      "id": "...",
      "base_version": 3,
      "deleted": false,
      "ciphertext": "...",
      "nonce": "..."
    }
  ]
}
```

返回 accepted + conflicts。单批建议 <= 500 条，payload <= 5 MiB。

## 6. 冲突策略

不要在服务器上解密业务字段。服务端只做版本 CAS。

客户端冲突：

- 无本地未提交变化：接受云端。
- 同一对象双方都改：生成 conflict UI。
- 可自动合并的无关对象直接合并。
- Proxy Slot 的 `local_port` 冲突不可自动随便改，必须提示。

## 7. Backup/Revision

服务端的 cursor 日志天然提供历史；v1 可提供“最近 30 个快照”的客户端功能：客户端定期生成一个加密 manifest record。恢复前自动本地备份。

## 8. HTTP 细节

- 全部 JSON。
- 请求 ID：`X-Request-Id`。
- 客户端版本：`X-Node2Socks-Version`。
- API 兼容：`server-info.api_version`。
- 反代后只信任 HTTPS；客户端 release 默认拒绝纯 HTTP，`localhost`/开发模式除外。
- 所有错误返回稳定 machine code + human message。
