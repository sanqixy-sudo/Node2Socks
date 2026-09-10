import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import type { Action, LatencyJob, LatencyProgress } from "./types";

/** The backend owns the job; navigation and WebView recreation only reattach. */
export function useLatencyJob(action: Action) {
  const [progress, setProgress] = useState<LatencyProgress | null>(null);
  const [ready, setReady] = useState(false);
  const [starting, setStarting] = useState(false);
  const startPending = useRef(false);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let revision = 0;
    void (async () => {
      try {
        const off = await listen<LatencyProgress>("latency-progress", event => {
          if (disposed) return;
          revision++;
          setProgress(event.payload);
          if (event.payload.done) void action("读取测速结果", () => Promise.resolve(), { reload: "nodes", successNotice: false });
        });
        if (disposed) { off(); return; }
        unlisten = off;
        const before = revision;
        const result = await action("恢复测速进度", () => invoke<LatencyProgress | null>("latency_status"), { reload: "none", successNotice: false });
        if (!disposed) {
          if (result.ok && revision === before) setProgress(result.value ?? null);
          setReady(result.ok);
        }
      } catch (error) {
        if (!disposed) void action("恢复测速进度", () => Promise.reject(error), { reload: "none" });
      }
    })();
    return () => { disposed = true; unlisten?.(); };
  }, [action]);

  const running = !!progress && !progress.done;
  const runTest = useCallback(async (nodeIds: string[]) => {
    if (!ready || startPending.current || running) return;
    startPending.current = true;
    setStarting(true);
    try {
      const result = await action("开始节点测速", () => invoke<LatencyJob>("start_latency_test", { nodeIds }), { reload: "none", successNotice: false });
      if (result.ok) setProgress(current => current?.jobId === result.value.jobId ? current : {
        jobId: result.value.jobId, total: result.value.total, completed: 0, done: false, cancelled: false,
      });
    } finally { startPending.current = false; setStarting(false); }
  }, [action, ready, running]);
  const cancel = useCallback(async () => {
    if (progress && !progress.done) await action("取消节点测速", () => invoke("cancel_latency_test", { jobId: progress.jobId }), { reload: "none" });
  }, [action, progress]);
  return { progress, runTest, cancel, testing: !ready || starting || !!progress && !progress.done };
}
