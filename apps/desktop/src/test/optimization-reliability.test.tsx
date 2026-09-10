import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useLatencyJob } from "../product/useLatencyJob";
import { useSettingsDraft } from "../product/useSettingsDraft";
import { customPortError } from "../product/slotPort";
import { defaultSettings } from "../product/types";
import type { Action, LatencyProgress, Slot } from "../product/types";

const { invokeMock, events } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  events: { callback: undefined as undefined | ((event: { payload: LatencyProgress }) => void) },
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_name, callback) => {
    events.callback = callback;
    return () => { if (events.callback === callback) events.callback = undefined; };
  }),
}));
const action: Action = async (_label, run) => {
  try { return { ok: true, value: await run() }; }
  catch (error) { return { ok: false, error: String(error) }; }
};
beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (command, args) => command === "update_settings" ? { value: args.settings } : null);
});
afterEach(cleanup);

function useSettings() {
  const [settings, setSettings] = useState(defaultSettings);
  return useSettingsDraft(settings, setSettings, action);
}

it("saves appearance without submitting or discarding port/network drafts", async () => {
  const { result } = renderHook(useSettings);
  act(() => result.current.setDraft(current => ({ ...current, portStart: 9, outboundMode: "manual", outboundInterface: "Ethernet" })));
  await act(async () => result.current.immediate("theme", "dark"));
  expect(invokeMock).toHaveBeenCalledWith("update_settings", { settings: { ...defaultSettings, theme: "dark" } });
  expect(result.current.draft).toMatchObject({ theme: "dark", portStart: 9, outboundMode: "manual" });
});

it("saves ports and network independently using the latest committed settings", async () => {
  const { result } = renderHook(useSettings);
  act(() => result.current.setDraft(current => ({ ...current, portStart: 22000, portEnd: 22100, outboundMode: "manual", outboundInterface: "Ethernet" })));
  await act(async () => result.current.savePorts());
  expect(invokeMock).toHaveBeenLastCalledWith("update_settings", { settings: { ...defaultSettings, portStart: 22000, portEnd: 22100 } });
  await act(async () => result.current.saveNetwork());
  expect(invokeMock).toHaveBeenLastCalledWith("update_settings", { settings: {
    ...defaultSettings, portStart: 22000, portEnd: 22100, outboundMode: "manual", outboundInterface: "Ethernet",
  } });
});

it("rolls back only the failed group and keeps unrelated drafts", async () => {
  const { result } = renderHook(useSettings);
  act(() => result.current.setDraft(current => ({ ...current, portStart: 22000, portEnd: 22100, outboundMode: "auto" })));
  invokeMock.mockRejectedValueOnce(new Error("database unavailable"));
  await act(async () => result.current.savePorts());
  expect(result.current.draft).toMatchObject({ portStart: 21000, portEnd: 21999, outboundMode: "auto" });
});

it("blocks overlapping saves while a group is still committing", async () => {
  let resolve!: (value: unknown) => void;
  invokeMock.mockImplementationOnce(() => new Promise(done => { resolve = done; }));
  const { result } = renderHook(useSettings);
  act(() => { result.current.immediate("theme", "dark"); result.current.savePorts(); });
  expect(invokeMock).toHaveBeenCalledTimes(1);
  expect(result.current.saving).toBe(true);
  await act(async () => resolve({ value: { ...defaultSettings, theme: "dark" } }));
  expect(result.current.saving).toBe(false);
});

const progress: LatencyProgress = { jobId: "running-job", completed: 4, total: 20, done: false, cancelled: false };

it("restores a backend job after navigation and can cancel that same job", async () => {
  invokeMock.mockResolvedValue(progress);
  const first = renderHook(() => useLatencyJob(action));
  await waitFor(() => expect(first.result.current.progress?.completed).toBe(4));
  first.unmount();
  const second = renderHook(() => useLatencyJob(action));
  await waitFor(() => expect(second.result.current.progress?.jobId).toBe("running-job"));
  expect(second.result.current.testing).toBe(true);
  await act(async () => second.result.current.runTest(["node"]));
  expect(invokeMock).not.toHaveBeenCalledWith("start_latency_test", expect.anything());
  await act(async () => second.result.current.cancel());
  expect(invokeMock).toHaveBeenCalledWith("cancel_latency_test", { jobId: "running-job" });
});

it("never overwrites newer progress events with a late status response", async () => {
  let resolve!: (value: unknown) => void;
  invokeMock.mockImplementationOnce(() => new Promise(done => { resolve = done; }));
  const { result } = renderHook(() => useLatencyJob(action));
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("latency_status"));
  act(() => events.callback?.({ payload: { ...progress, completed: 9 } }));
  await act(async () => resolve(progress));
  expect(result.current.progress?.completed).toBe(9);
});

it("preserves a fast completion event arriving before start returns", async () => {
  invokeMock.mockImplementation(async command => {
    if (command === "latency_status") return null;
    events.callback?.({ payload: { ...progress, completed: 20, done: true } });
    return { jobId: progress.jobId, total: 20 };
  });
  const { result } = renderHook(() => useLatencyJob(action));
  await waitFor(() => expect(result.current.testing).toBe(false));
  await act(async () => result.current.runTest(["node"]));
  expect(result.current.progress?.done).toBe(true);
  expect(result.current.testing).toBe(false);
});

it("prevents double starts before the first response", async () => {
  let resolve!: (value: unknown) => void;
  invokeMock.mockImplementation(command => command === "latency_status" ? Promise.resolve(null) : new Promise(done => { resolve = done; }));
  const { result } = renderHook(() => useLatencyJob(action));
  await waitFor(() => expect(result.current.testing).toBe(false));
  act(() => { void result.current.runTest(["node"]); void result.current.runTest(["node"]); });
  expect(invokeMock.mock.calls.filter(([command]) => command === "start_latency_test")).toHaveLength(1);
  await act(async () => resolve({ jobId: progress.jobId, total: 1 }));
});

it("validates explicit ports against range, syntax, occupied slots and node count", () => {
  const used: Slot[] = [{ id: "slot", name: "slot", port: 21001, state: "active" }];
  expect(customPortError("", defaultSettings, used, 6)).toBeNull();
  expect(customPortError("21000", defaultSettings, used, 1)).toBeNull();
  expect(customPortError("21999", defaultSettings, used, 1)).toBeNull();
  for (const input of ["21000.5", "2.1e4", "-1", "abc"]) expect(customPortError(input, defaultSettings, used, 1)).toBe("请输入整数端口");
  for (const input of ["20999", "22000", "99999999999999999999"]) expect(customPortError(input, defaultSettings, used, 1)).toContain("范围");
  expect(customPortError("21001", defaultSettings, used, 1)).toContain("使用");
  expect(customPortError("21002", defaultSettings, used, 2)).toContain("一个节点");
});
