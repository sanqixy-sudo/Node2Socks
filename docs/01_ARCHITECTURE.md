# 01 — 总体技术架构

## 1. 总览

```text
┌──────────────────────── Node2Socks Desktop ────────────────────────┐
│ React UI                                                           │
│   ↓ Tauri commands/events                                          │
│ Rust Application Service                                           │
│   ├─ Subscription Service                                          │
│   ├─ Node Catalog                                                  │
│   ├─ Proxy Slot Service                                            │
│   ├─ Cloud Sync Client                                             │
│   ├─ Diagnostics / Clash Detector                                  │
│   ├─ SQLite                                                        │
│   └─ ProxyCoreAdapter                                              │
│          ↓                                                         │
│   node2socks-mihomo.exe (separate process)                         │
│          ├─ 127.0.0.1:21001 -> selector slot-001 -> node JP        │
│          ├─ 127.0.0.1:21002 -> selector slot-002 -> node US        │
│          └─ localhost external-controller (random port + secret)   │
└────────────────────────────────────────────────────────────────────┘
                     │
                     │ HTTPS，仅配置同步
                     ▼
┌──────────────────── Node2Socks Cloud ──────────────────────────────┐
│ Axum API                                                           │
│ Auth / Devices / Sync Records / Revisions                          │
│ SQLite WAL (/data/node2socks-cloud.db)                             │
│ Docker container                                                   │
└────────────────────────────────────────────────────────────────────┘
```

## 2. 目录建议

```text
node2socks/
├─ AGENTS.md
├─ Cargo.toml                  # Rust workspace
├─ package.json
├─ pnpm-workspace.yaml
├─ apps/
│  └─ desktop/
│     ├─ src/                  # React
│     └─ src-tauri/            # Tauri shell
├─ crates/
│  ├─ domain/                  # Node/Slot/Subscription shared models
│  ├─ storage/                 # SQLite repositories + migrations
│  ├─ core-adapter/            # ProxyCore trait + Mihomo implementation
│  ├─ subscriptions/           # fetch/detect/normalize/provider bridge
│  ├─ sync-client/
│  ├─ crypto/
│  ├─ diagnostics/
│  └─ common/
├─ services/
│  └─ cloud/
│     ├─ src/
│     ├─ migrations/
│     ├─ Dockerfile
│     └─ docker-compose.yml
├─ sidecar/
│  ├─ windows-x64/
│  │  └─ node2socks-mihomo.exe
│  └─ LICENSES/
├─ fixtures/
└─ docs/
```

## 3. Domain 层必须与 Mihomo 解耦

Domain 不应该出现 Mihomo YAML 字段。核心对象：

- `Subscription`
- `Node`
- `ProxySlot`
- `SlotBinding`
- `AppSettings`
- `CloudProfile`
- `Device`
- `SyncRecord`

通过 `ProxyCore` 接口调用：

```rust
trait ProxyCore {
    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn health(&self) -> Result<CoreHealth>;
    async fn apply_topology(&self, topology: CoreTopology) -> Result<()>;
    async fn refresh_provider(&self, provider_id: &str) -> Result<()>;
    async fn select_for_slot(&self, slot_id: Uuid, internal_node: &str) -> Result<()>;
    async fn block_slot(&self, slot_id: Uuid) -> Result<()>;
    async fn test_node(&self, internal_node: &str, target: &Url) -> Result<Latency>;
}
```

这样将来可增加 SingBoxAdapter，而不改产品数据模型。

## 4. 运行目录隔离

Windows 建议：

```text
%LOCALAPPDATA%\Node2Socks\
  app.db
  runtime\
    mihomo\
      config.yaml
      cache.db
      providers\
  logs\
  backups\
```

禁止读写用户 Clash Verge 的 AppData 目录。

## 5. 生命周期

启动顺序：

1. 打开 SQLite，执行 migrations。
2. 读取 settings / slots / subscriptions。
3. 启动本地 provider bridge。
4. 生成 Mihomo runtime config。
5. 检查 Slot 端口冲突。
6. 启动 Mihomo sidecar。
7. 等待 Controller healthy。
8. 刷新 provider / 查询节点目录。
9. 将每个 Slot 恢复到保存的绑定；缺失节点设置 BLOCK。
10. 启动订阅定时器与云同步 worker。
11. UI 标记 Ready。

退出顺序：停止新任务 → flush DB/sync queue → 停 Core → 关闭 provider bridge → 退出。
