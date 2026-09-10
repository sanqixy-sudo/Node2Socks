import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { createRefreshQueue, retainSnapshot } from "../product/snapshotRefresh";
import { useSnapshots } from "../product/useSnapshots";
import { defaultSettings, emptyDashboard } from "../product/types";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));
afterEach(() => { cleanup(); invokeMock.mockReset(); });

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>(done => { resolve = done; });
  return { promise, resolve };
}

it("merges bursts but retains all requested scopes", async () => {
  const load = vi.fn(async () => {});
  const refresh = createRefreshQueue(load);
  await Promise.all([refresh("dashboard"), refresh("nodes"), refresh("dashboard")]);
  expect(load.mock.calls).toEqual([[["dashboard", "nodes"]]]);
});

it("reads again for a mutation that finishes during an existing refresh", async () => {
  const first = deferred();
  const load = vi.fn().mockImplementationOnce(() => first.promise).mockResolvedValue(undefined);
  const refresh = createRefreshQueue(load);
  const beforeMutation = refresh("dashboard");
  await Promise.resolve();
  const afterMutation = refresh("dashboard");
  first.resolve();
  await Promise.all([beforeMutation, afterMutation]);
  expect(load.mock.calls).toEqual([[["dashboard"]], [["dashboard"]]]);
});

it("recovers after a failed refresh", async () => {
  const load = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue(undefined);
  const refresh = createRefreshQueue(load);
  await expect(refresh("nodes")).rejects.toThrow("offline");
  await expect(refresh("nodes")).resolves.toBeUndefined();
  expect(load).toHaveBeenCalledTimes(2);
});

it("retains equal snapshots but applies changes", () => {
  const original = [{ id: "node", latency: 20 }];
  expect(retainSnapshot(original, [{ id: "node", latency: 20 }])).toBe(original);
  expect(retainSnapshot(original, [{ id: "node", latency: 21 }])).not.toBe(original);
});

it("keeps a 1000-node list stable over ten idle refreshes", async () => {
  const nodes = Array.from({ length: 1000 }, (_, index) => ({
    id: String(index), subscriptionId: "sub", subscriptionName: "测试",
    displayName: "节点 " + index, protocol: "ss", present: true, boundSlots: [],
  }));
  invokeMock.mockImplementation(async command => structuredClone(
    command === "dashboard_snapshot" ? emptyDashboard : command === "list_node_views" ? nodes : defaultSettings
  ));
  const { result } = renderHook(() => useSnapshots());
  await act(async () => { await result.current.refresh(); });
  const before = result.current;
  for (let index = 0; index < 10; index++) {
    await act(async () => { await result.current.refreshLive(); });
  }
  expect(result.current.nodes).toBe(before.nodes);
  expect(result.current.data).toBe(before.data);
  expect(result.current.settings).toBe(before.settings);
  invokeMock.mockImplementation(async command => command === "dashboard_snapshot"
    ? { ...emptyDashboard, coreRunning: true }
    : nodes.map((node, index) => index === 0 ? { ...node, latencyMs: 50 } : node));
  await act(async () => { await result.current.refreshLive(); });
  expect(result.current.data.coreRunning).toBe(true);
  expect(result.current.nodes[0].latencyMs).toBe(50);
});
