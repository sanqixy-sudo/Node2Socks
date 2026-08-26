# 09 — 测试与验收

## P0 单元测试

- Port allocator：分配、冲突、cooldown、固定同步端口。
- Node stable key：名字变化/参数变化。
- Subscription detection：YAML/URI/Base64/坏数据/超大数据。
- DB migrations。
- Encryption envelope roundtrip + tamper detection。
- Sync version conflict。

## P1 Core POC 必过

在一台 Windows 机器上：

1. Node2Socks 启动自己的 Mihomo。
2. 创建 `127.0.0.1:21001` 和 `21002`。
3. 绑定两个不同上游节点。
4. 分别通过两个 SOCKS 请求出口 IP，必须不同且符合节点预期。
5. 用户的 Clash 关闭时仍可工作（网络条件允许）。
6. Clash System Proxy 开启/切节点时，两个 Slot 出口不跟随变化。
7. 重启 Node2Socks，端口与绑定恢复。

## P2 订阅刷新

- 同一订阅重新排序，Slot 不变化。
- 节点新增，旧 Slot 不变化。
- 未绑定节点删除，无影响。
- 已绑定节点删除：对应 Slot 必须 BLOCK，不能自动切到别的节点。
- 节点恢复：可恢复原绑定或要求明确动作，不能无提示错绑。

## P3 端口

- 21001 被第三方占用：Node2Socks 启动提示冲突，不改 21001。
- 用户结束占用进程后可重试。
- 删除 Slot 后端口 cooldown。
- 100 个 Slot 启动和恢复压力测试。

## P4 Cloud

- 本地模式从未登录，所有代理功能可用。
- Docker cloud 全新部署。
- 客户端填自定义域名注册/登录。
- A 机创建订阅 + Slot；B 机登录恢复相同端口。
- B 机改绑定，A 机同步。
- 云服务停机时 A/B 本地代理不中断。
- 离线修改恢复联网后同步。
- 同一 Slot 双端修改产生冲突，不静默覆盖。
- 重装模拟：清空本地数据后登录恢复。

## P5 Clash TUN

必须在至少 Clash Verge Rev + Mihomo TUN 的真实环境实测：

- 自动物理网卡绑定。
- 手动物理网卡绑定。
- 切换 Wi-Fi/有线。
- sleep/wake。

如果某环境无法可靠绕过 TUN，产品必须诚实提示“检测到透明代理可能影响 Node2Socks 出站”，不能伪报独立。

## Release Gate

- 0 个已知会导致 Slot 静默换节点的 bug。
- 0 个敏感信息明文日志。
- 关键 DB migration 可从上一版本升级。
- Core checksum 验证通过。
- Windows installer clean VM 安装/卸载/重装通过。
