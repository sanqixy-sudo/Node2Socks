import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ToastNotice } from "../product/AppShell";
import { CloudPage, NodesPage, SettingsPage, SlotsPage, SubscriptionsPage } from "../product/pages";
import type { Action, AppSettings, CloudStatus, Dashboard, NodeView } from "../product/types";
import { defaultSettings, emptyDashboard } from "../product/types";

const { invokeMock, listenMock } = vi.hoisted(() => ({ invokeMock: vi.fn(), listenMock: vi.fn(async () => () => {}) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ isMaximized: vi.fn(async () => false), onResized: vi.fn(async () => () => {}), toggleMaximize: vi.fn(), minimize: vi.fn(), close: vi.fn() }) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

const action: Action = async (_label, run) => {
  try { return { ok: true, value: await run() }; }
  catch (error) { return { ok: false, error: String(error) }; }
};
const failedAction: Action = async () => ({ ok: false, error: "保存失败" });
const dashboard = (overrides: Partial<Dashboard> = {}): Dashboard => ({ ...emptyDashboard, ...overrides, diagnostics: { ...emptyDashboard.diagnostics, ...overrides.diagnostics } });
const node = (id: string, name: string, subscriptionName = "订阅 A"): NodeView => ({ id, subscriptionId: subscriptionName, subscriptionName, displayName: name, protocol: "ss", present: true, boundSlots: [] });

beforeEach(() => { invokeMock.mockReset(); invokeMock.mockResolvedValue(undefined); listenMock.mockClear(); });
afterEach(() => { cleanup(); vi.useRealTimers(); });

describe("reliable desktop interactions", () => {
  it("keeps the subscription drawer open after a failed save and closes it after success", async () => {
    invokeMock.mockRejectedValueOnce(new Error("数据库不可写"));
    const view = render(<SubscriptionsPage data={dashboard()} action={action} />);
    fireEvent.click(screen.getByRole("button", { name: "添加订阅" }));
    fireEvent.change(screen.getByLabelText("名称"), { target: { value: "测试订阅" } });
    fireEvent.change(screen.getByLabelText("订阅 URL"), { target: { value: "https://example.com/sub" } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(screen.getByRole("dialog")).toBeInTheDocument());
    view.unmount();

    invokeMock.mockResolvedValueOnce({ value: "subscription-id" });
    render(<SubscriptionsPage data={dashboard()} action={action} />);
    fireEvent.click(screen.getByRole("button", { name: "添加订阅" }));
    fireEvent.change(screen.getByLabelText("名称"), { target: { value: "测试订阅" } });
    fireEvent.change(screen.getByLabelText("订阅 URL"), { target: { value: "https://example.com/sub" } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("submits only currently filtered nodes to latency testing", async () => {
    invokeMock.mockResolvedValueOnce({ jobId: "job-1", total: 1 });
    const nodes = [node("node-a", "香港 A"), node("node-b", "日本 B")];
    render(<NodesPage data={dashboard({ coreRunning: true })} nodes={nodes} settings={defaultSettings} onSettings={() => {}} action={action} />);
    fireEvent.change(screen.getByPlaceholderText("搜索节点或订阅"), { target: { value: "日本" } });
    fireEvent.click(screen.getByRole("button", { name: "测速当前结果" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("start_latency_test", { nodeIds: ["node-b"] }));
  });

  it("shows every Slot bound to the same node", () => {
    const bound = { ...node("node-a", "香港 A"), boundSlots: [{ id: "s1", port: 21000, name: "店铺 A" }, { id: "s2", port: 21001, name: "店铺 B" }] };
    render(<NodesPage data={dashboard({ coreRunning: true })} nodes={[bound]} settings={defaultSettings} onSettings={() => {}} action={action} />);
    expect(screen.getByText("2 个 Slot")).toBeInTheDocument();
    expect(screen.getByText(/Slot 21000、Slot 21001/)).toBeInTheDocument();
  });

  it("rolls an immediate setting back when persistence fails", async () => {
    const onSettings = vi.fn();
    render(<SettingsPage settings={defaultSettings} onSettings={onSettings} data={dashboard()} action={failedAction} />);
    const theme = screen.getByDisplayValue("跟随系统") as HTMLSelectElement;
    fireEvent.change(theme, { target: { value: "dark" } });
    await waitFor(() => expect(theme.value).toBe("system"));
    expect(onSettings).toHaveBeenLastCalledWith(defaultSettings);
  });

  it("restores the configured cloud account when the page opens", async () => {
    const cloud: CloudStatus = { configured: true, loggedIn: true, baseUrl: "https://sync.example.com", accountName: "owner@example.com", deviceId: "device-1", pendingCount: 2, failedCount: 0 };
    invokeMock.mockImplementation((command: string) => command === "cloud_status" ? Promise.resolve(cloud) : Promise.resolve([]));
    render(<CloudPage action={action} />);
    expect(await screen.findByText("owner@example.com")).toBeInTheDocument();
    expect(screen.getByDisplayValue("https://sync.example.com")).toBeDisabled();
  });

  it("keeps hidden Slot selections explicit after filtering", () => {
    render(<SlotsPage data={dashboard({ slots: [
      { id: "slot-a", name: "香港店铺", port: 21000, nodeName: "香港 A", nodeId: "node-a", state: "active" },
      { id: "slot-b", name: "日本店铺", port: 21001, nodeName: "日本 B", nodeId: "node-b", state: "active" },
    ] })} nodes={[node("node-a", "香港 A"), node("node-b", "日本 B")]} action={action} />);
    fireEvent.click(screen.getByLabelText("选择端口 21000"));
    fireEvent.change(screen.getByPlaceholderText("搜索端口、名称或节点"), { target: { value: "日本" } });
    expect(screen.getByText(/已选 1（含隐藏 1）/)).toBeInTheDocument();
  });

  it("auto dismisses notifications and pauses the timer while focused or hovered", () => {
    vi.useFakeTimers();
    const onClose = vi.fn();
    render(<ToastNotice kind="ok" text="保存成功" onClose={onClose} />);
    const toast = screen.getByRole("status");
    act(() => { vi.advanceTimersByTime(1000); fireEvent.pointerEnter(toast); vi.advanceTimersByTime(8000); });
    expect(onClose).not.toHaveBeenCalled();
    act(() => { fireEvent.pointerLeave(toast); vi.advanceTimersByTime(2499); });
    expect(onClose).not.toHaveBeenCalled();
    act(() => { vi.advanceTimersByTime(1); });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});