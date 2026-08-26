# 04 — 数据模型

## 1. 本地 SQLite 核心表

### subscriptions

```text
id UUID PK
name TEXT
url_cipher BLOB
refresh_interval_sec INTEGER
headers_cipher BLOB NULL
enabled BOOL
content_format TEXT NULL
etag TEXT NULL
last_modified TEXT NULL
last_success_at DATETIME NULL
last_error TEXT NULL
cached_payload_cipher BLOB NULL
created_at
updated_at
sync_version INTEGER
```

### nodes

```text
id UUID PK
subscription_id UUID FK
stable_key TEXT
internal_name TEXT
upstream_name TEXT
protocol TEXT
provider_name TEXT
last_seen_at DATETIME
is_present BOOL
metadata_json TEXT
created_at
updated_at
UNIQUE(subscription_id, stable_key)
```

### proxy_slots

```text
id UUID PK
name TEXT
local_port INTEGER UNIQUE
listen_host TEXT DEFAULT '127.0.0.1'
username_cipher BLOB NULL
password_cipher BLOB NULL
enabled BOOL
created_at
updated_at
sync_version INTEGER
```

### slot_bindings

```text
slot_id UUID PK/FK
node_id UUID NULL
state TEXT  # active/orphaned/unbound/blocked/error
last_applied_internal_name TEXT NULL
updated_at
sync_version INTEGER
```

### app_settings

key/value，区分 `scope = synced | device_local`。

### cloud_profiles

```text
id UUID
base_url TEXT
account_name TEXT
device_id UUID
is_active BOOL
last_cursor INTEGER
```

access/refresh token 不直接明文放 SQLite，使用 OS keychain/DPAPI。

### sync_outbox

```text
id INTEGER PK AUTOINCREMENT
record_type TEXT
record_id UUID
operation TEXT  # upsert/delete
base_version INTEGER
payload_cipher BLOB
created_at
attempts INTEGER
last_error TEXT NULL
```

## 2. 稳定 ID

### Slot

永远由随机 UUID 定义，端口只是属性。同步时以 `slot_id` 合并。

### Subscription

随机 UUID；URL 变化仍可视为同一 Subscription。

### Node

v1：

```text
stable_key = normalized(provider-specific identity)
```

优先级：

1. 能解析 URI 时：协议 + server + port + auth identity + transport/SNI/reality key 等做 hash，忽略 display name。
2. YAML 能读取关键连接字段时同理。
3. 无法可靠解析时：subscription_id + normalized upstream node name。

节点名字变化但连接参数不变时尽量保持 Node ID；连接参数真的变化时允许产生新 Node，并把旧绑定标记 orphaned，不静默迁移。

## 3. 删除语义

云同步对象使用 tombstone，不物理立即删除。保留至少 30 天，便于多设备离线后同步删除操作。

## 4. 版本

每个同步实体有 `version`。服务端成功 upsert 后 version +1。客户端上传时携带 `base_version`，不一致返回 409 conflict。
