# 06 — 与本机 Clash / Clash Verge 共存

## 1. 两种情况

### System Proxy

Clash 只设置 Windows 系统 HTTP/SOCKS Proxy 时，Node2Socks 的 Rust HTTP client 与 Mihomo Core 默认不读取系统代理即可保持独立。唯一例外：单个订阅的下载网络显式选择「使用系统代理」时，仅该订阅的下载请求走系统代理，Proxy Core 出站不受影响。类似地，「通过节点下载」模式只让该订阅的下载请求经本机 Core 的专用下载通道（selector `n2s-download` + 仅 127.0.0.1 的 SOCKS 监听）从所选节点拨号，不影响任何 Slot 的路由与绑定。

验收：关闭 Clash 后 Slot 仍能访问；打开 Clash 并切换节点后，Slot 的出口 IP 不变化。

### TUN

Clash TUN 是网络层透明接管，可能捕获 `node2socks-mihomo.exe` 到机场服务器的连接，形成：

```text
Node2Socks Core -> Clash TUN -> Clash node -> Node2Socks upstream
```

因此不能简单宣称“两个进程就一定独立”。

## 2. v1 共存策略

### A. 检测

尽可能检测：

- Clash Verge / mihomo / clash 进程。
- Windows 虚拟网卡名称和默认路由。
- 常见 TUN adapter。
- 当前物理默认网卡。

UI 显示：`Clash 未检测 / System Proxy / 疑似 TUN`。

### B. Node2Socks 自己的出站网卡

Mihomo 支持 `interface-name`。提供：

```text
出站网络
(•) 自动选择物理默认网卡
( ) 跟随系统路由
( ) 手动选择网卡: Ethernet / Wi-Fi
```

当 TUN 存在时，优先尝试把 Node2Socks Mihomo outbound 绑定真实物理接口。Windows 环境必须实测，不能仅凭配置推断已绕过。

### C. 第三方 Clash 绕过提示

如果绑定物理接口仍被透明接管，UI 可给出“在你的 Clash 中把 `node2socks-mihomo.exe` 设为 DIRECT”的操作建议。不要未经许可改第三方配置。

进程规则示意仅作说明：

```text
PROCESS-NAME,node2socks-mihomo.exe,DIRECT
```

具体语法由用户所用 Clash 前端/配置决定。

## 3. 订阅下载

Subscription fetch 默认用显式 `no_proxy` client / direct socket，不继承系统 HTTP proxy。

高级设置可允许：

- Direct（默认）
- Custom HTTP/SOCKS proxy（只用于下载订阅）

不要把“订阅下载需要本地 Clash”与“代理核心流量经 Clash”混在一起。

## 4. 链路诊断

提供诊断按钮：

```text
Slot 21001
Core: running
Bound node: JP-01
SOCKS listen: OK
Node delay: 82 ms
Exit IP via slot: x.x.x.x
Selected outbound adapter: Wi-Fi
Clash/TUN: detected / not detected
```

不要显示无法证明的“100% 未经过 Clash”。如果只能确认绑定了物理网卡，就写“已绑定物理网卡”；链路隔离以测试结果为准。

## 5. 验收矩阵

- Clash completely off。
- Clash System Proxy on。
- Clash System Proxy node switched repeatedly。
- Clash TUN on + automatic adapter。
- Clash TUN on + manual physical adapter。
- Sleep/wake。
- Wi-Fi/Ethernet switch。

每个场景检查 Slot 是否仍是预期出口 IP。
