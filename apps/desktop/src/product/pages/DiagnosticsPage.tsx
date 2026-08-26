import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import type { Action, Dashboard } from "../types";
import { Badge, Button, PageHeader, Section } from "../ui";

export function DiagnosticsPage({ data, action }: { data: Dashboard; action: Action }) {
  const values = [
    { name: "Core", value: data.diagnostics.core, tone: data.coreRunning ? "success" : "neutral", label: data.coreRunning ? "正常" : "已停止" },
    { name: "监听地址", value: "127.0.0.1 · 仅本机", tone: "info", label: "安全" },
    { name: "Clash", value: data.diagnostics.clash, tone: data.diagnostics.clash.includes("未检测") ? "neutral" : "info", label: "信息" },
    { name: "系统代理", value: data.diagnostics.systemProxy, tone: data.diagnostics.systemProxy.includes("未启用") ? "success" : "info", label: "信息" },
    { name: "TUN", value: data.diagnostics.tun, tone: data.diagnostics.tun.includes("未检测") ? "neutral" : "info", label: "信息" },
    { name: "出站网卡", value: data.diagnostics.outboundAdapter, tone: "info", label: "信息" },
  ] as const;
  const exportReport = async () => {
    const destination = await save({ defaultPath: "node2socks-diagnostics.txt", filters: [{ name: "诊断报告", extensions: ["txt"] }] });
    if (destination) await action("导出诊断", () => invoke("diagnostic_export", { destination }), { reload: "none" });
  };
  return <>
    <PageHeader title="诊断" description="检测 Node2Socks、Clash、系统代理和本地链路状态" actions={<>
      <Button icon="refresh" onClick={() => void action("重新读取状态", () => invoke("dashboard_snapshot"), { reload: "dashboard" })}>重新检测</Button>
      <Button icon="download" kind="primary" onClick={() => void exportReport()}>导出诊断</Button>
    </>} />
    <Section>
      <div className="diagnostic-grid">{values.map(item => <article key={item.name}><span><strong>{item.name}</strong><small>{item.value}</small></span><Badge tone={item.tone}>{item.label}</Badge></article>)}</div>
      {data.diagnostics.warning && <details className="diagnostic-details"><summary>查看网络共存警告</summary><p>{data.diagnostics.warning}</p><small>可在 Clash 中为 node2socks-mihomo.exe 添加 PROCESS-NAME DIRECT；本软件不会擅自修改 Clash。</small></details>}
    </Section>
  </>;
}