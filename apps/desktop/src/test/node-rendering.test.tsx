import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { NodesPage } from "../product/pages/NodesPage";
import { defaultSettings, emptyDashboard } from "../product/types";
import type { Action, LatencyProgress } from "../product/types";

const { events } = vi.hoisted(() => ({
  events: { progress: undefined as undefined | ((event: { payload: LatencyProgress }) => void) },
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string) => command === "latency_status" ? null : ({ jobId: "job", total: 1000 })),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_name, callback) => { events.progress = callback; return () => { events.progress = undefined; }; }),
}));
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

it("updates progress without rebuilding 1000 unchanged node cards", async () => {
  const action: Action = async (_label, run) => ({ ok: true, value: await run() });
  const nodes = Array.from({ length: 1000 }, (_, index) => ({
    id: String(index), displayName: "节点 " + index, protocol: "ss", present: true,
    subscriptionId: "sub", subscriptionName: "测试", boundSlots: [],
    latencyCheckedAt: Math.floor(Date.now() / 1000), latencyMs: 50,
  }));
  render(<NodesPage data={{ ...emptyDashboard, coreRunning: true }} nodes={nodes}
    settings={defaultSettings} onSettings={() => {}} action={action} />);
  await waitFor(() => expect(screen.getByRole("button", { name: "测速当前结果" })).not.toBeDisabled());
  fireEvent.click(screen.getByRole("button", { name: "测速当前结果" }));
  await waitFor(() => expect(screen.getByText("0/1000")).toBeInTheDocument());
  const formatTime = vi.spyOn(Date.prototype, "toLocaleString");
  for (let completed = 1; completed <= 10; completed++) {
    act(() => events.progress?.({ payload: { jobId: "job", completed, total: 1000, done: false, cancelled: false } }));
  }
  expect(screen.getByText("10/1000")).toBeInTheDocument();
  expect(formatTime).not.toHaveBeenCalled();
}, 30000);
