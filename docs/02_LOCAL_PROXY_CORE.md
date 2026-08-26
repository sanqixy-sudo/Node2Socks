# 02 — 本地代理 Core、订阅和 Proxy Slot

## 1. 为什么使用 Mihomo sidecar

Mihomo 已支持多 Listener、HTTP/SOCKS/Mixed inbound、多种上游协议、Proxy Provider、Selector、Controller API。Node2Socks 不重新实现代理协议，只做产品层。

固定版本起点：**Mihomo v1.19.30 (2026-08-16 stable)**。Windows amd64 官方通用包 SHA-256：

```text
22c09fd67673895ef7cd6b1820563918275c3d316f2462b306208675118db3c0
```

开发时可更新，但每个发布版本必须 pin 版本和 checksum，先跑回归测试。

## 2. 一 Slot 一 SOCKS Listener

概念配置：

```yaml
external-controller: 127.0.0.1:19090
secret: "<random-at-launch>"
allow-lan: false

listeners:
  - name: slot-001-in
    type: socks
    listen: 127.0.0.1
    port: 21001
    proxy: slot-001
    udp: true

  - name: slot-002-in
    type: socks
    listen: 127.0.0.1
    port: 21002
    proxy: slot-002
    udp: true

proxy-groups:
  - name: slot-001
    type: select
    proxies: [REJECT]
    default-selected: REJECT
    empty-fallback: REJECT
    use: [sub-a, sub-b]

  - name: slot-002
    type: select
    proxies: [REJECT]
    default-selected: REJECT
    empty-fallback: REJECT
    use: [sub-a, sub-b]

```

关键：Slot 的 listener 和 selector 名称由 `slot_id` 派生，不依赖节点。

## 3. 订阅 Provider Bridge

不要让“代理流量”经过本机 Clash。订阅下载也默认 DIRECT，但可提供“仅订阅下载使用指定 HTTP/SOCKS 代理”的高级选项。

推荐每个 Subscription 一个 Mihomo Provider：

```yaml
proxy-providers:
  sub-a:
    type: http
    url: http://127.0.0.1:17890/provider/<subscription_uuid>?token=<local_secret>
    path: ./providers/sub-a.yaml
    interval: 86400
    override:
      additional-prefix: "[a1b2] "
    health-check:
      enable: true
      url: https://www.gstatic.com/generate_204
      interval: 300
```

Node2Socks 内部 provider bridge：

- 自己 fetch 机场订阅。
- 限制响应体大小（建议 10 MiB）。
- 30s 默认超时，可配置。
- 识别内容格式：YAML / URI / Base64 URI。
- 保存原始缓存和 ETag/Last-Modified。
- 内部 HTTP 仅监听 `127.0.0.1`，带随机 token。
- 返回 Mihomo 能接受的 provider 内容。
- 订阅更新后调用 Controller `PUT /providers/proxies/{provider}`，避免重启 Core。

Mihomo provider 支持 `override.additional-prefix`，用 subscription short id 给节点内部名字加前缀，避免两个机场都有“日本 01”造成冲突。

## 4. Node Catalog

更新 Provider 后查询：

```text
GET /providers/proxies/{provider_name}
```

保存：

- `subscription_id`
- `internal_name`：例如 `[a1b2] 日本 01`
- `display_name`：例如 `日本 01`
- `protocol/type`
- `last_seen_at`
- `available`
- `provider_name`

v1 stable node id 可以从 `subscription_id + normalized upstream name` 生成；同时为 URI 输入尽可能计算 `protocol endpoint fingerprint`。不要因为节点排序变化改变 ID。

## 5. Slot 绑定

用户把 `node_id` 绑定到 Slot：

1. 检查节点存在于当前 provider。
2. `PUT /proxies/slot-<id>`，body `{"name":"<internal_name>"}`。
3. API 返回 204 后写 DB binding。
4. UI 显示 active。

切换节点只走 Controller API，不改 listener 端口。

## 6. Fail-closed 是硬要求

风险：provider 更新后，当前 selector 选中的节点被删除，Mihomo 可能回退到组内其他成员。对于指纹浏览器这是不可接受的，因为会无声换 IP。

实现要求：

- 每个 selector 显式包含 `REJECT` 安全成员。
- 在固定 Mihomo 版本上写集成测试：被选节点消失时必须验证 selector 行为。
- Node2Socks provider 更新流程要在更新前计算“哪些 Slot 的绑定在新节点集合中不存在”。
- 对缺失绑定立即标记 `orphaned`，目标状态必须是 `REJECT`。
- 如果无法证明 Mihomo 的自动回退安全，更新流程宁愿短暂阻断相关 Slot，也不能自动落到其他活节点。

UI：

```text
21001   JP Tokyo 01   🔴 上游已不存在
[选择替代节点]  [解除绑定]
```

## 7. Port allocator

- 默认：21000–21999。
- Slot 删除后端口进入 cooldown（例如 10 分钟），避免刚释放就被重用导致误连。
- 云同步的 Slot 端口优先；本地发现冲突时不得自动改。
- 新 Slot 找第一个空闲端口。
- Windows 端口检测在启动 Core 前完成。

## 8. 拓扑变更与重载

- 节点切换：不重载 Core。
- 已有订阅内容更新：provider update，不重载 Core。
- 新增/删除订阅 provider：可能需要配置 reload。
- 新增/删除 Slot listener：可能需要配置 reload。

如果 Mihomo 当前版本支持稳定的动态 Listener 管理，可后续优化；v1 允许拓扑变化时短暂重载，但必须自动恢复原 Slot selection，且 UI 显示“重新加载核心”。

## 9. 出口测试

延迟和出口测试要分开：

- Latency：Mihomo `/proxies/{name}/delay`。
- Exit IP：通过目标 Slot 发起 HTTP 请求，如 Cloudflare trace / ipify。
- 显示 IP、国家、ASN/ISP（若使用可靠 API；可做可选）。

不要把 ICMP ping 当“节点可用”的唯一依据。
