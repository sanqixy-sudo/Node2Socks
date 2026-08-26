# Node2Socks UI 与绿色版复核 — 2026-08-26

## 复核结论

当前信息架构已经覆盖本地代理软件的核心任务：概览、订阅、节点、固定槽位、可选云同步、设置和诊断。首页先展示运行中的槽位、节点数量和核心状态，代理槽位页保留绑定/检测/删除入口，危险操作使用红色动作并要求二次确认；这些入口符合本地代理工具的使用顺序，没有增加账号或云端依赖。

本轮保留深色侧栏、白色内容卡片和蓝灰主色，收敛高饱和装饰，避免把代理状态误读成营销页面。后续拿到正式 Logo 后，只需替换品牌标记，不需要重做布局。

## 已实施的桌面 UI 改进

- 关闭 Tauri 系统装饰，增加 38px 的 Node2Socks 自定义顶栏。
- 顶栏包含统一风格的最小化、最大化/还原、关闭到托盘按钮；关闭按钮的可访问标签明确写为“关闭到托盘”。
- 顶栏可拖动，窗口操作通过 Tauri 窗口 API 执行，并补齐最小权限：`minimize`、`toggle-maximize`、`close`、`start-dragging`。
- 顶栏横跨整窗，侧栏从顶栏下方开始，避免出现系统标题栏与应用内容两套视觉。

## 绿色版

`scripts/build_portable.ps1` 会校验固定 Mihomo SHA-256，生成 `target/release/Node2Socks-Portable` 和 `Node2Socks-Portable-win-x64.zip`。绿色目录包含：

- `Node2Socks.exe`
- `node2socks-mihomo.exe`
- `portable.flag`
- `MIHOMO_NOTICE.md`
- `README.txt`

存在 `portable.flag` 时，数据库和 DPAPI 密钥写入绿色目录的 `data/`；没有该标记时仍使用 Windows 默认应用数据目录。这样既支持解压即用，也不会改变普通安装版的数据策略。

## 验证

- 前端 TypeScript/Vite production build：PASS。
- Rust `cargo check --package node2socks-desktop --locked`：PASS。
- 从最终绿色 ZIP 解压启动：进程存活、数据生成在解压目录 `data/`、工作集约 28.71 MB。
- 发送关闭请求后：进程仍存活、主窗口标题为空、WebView2 子进程为 0、工作集约 28.94 MB；清理后无残留进程。
- Windows 视觉自动化因 workspace deny-read ACL 无法运行，因此真实鼠标点击的截图复核标记为 `SKIPPED`；源码、构建和运行时行为已独立验证。
