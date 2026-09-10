import { invoke } from "@tauri-apps/api/core";
import { useCallback, useMemo, useState } from "react";
import type { AppSettings, Dashboard, NodeView } from "./types";
import { defaultSettings, emptyDashboard } from "./types";
import { createRefreshQueue, retainSnapshot } from "./snapshotRefresh";

export function useSnapshots() {
  const [data, setData] = useState<Dashboard>(emptyDashboard);
  const [nodes, setNodes] = useState<NodeView[]>([]);
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [connected, setConnected] = useState(false);
  const refresh = useMemo(() => createRefreshQueue(async scopes => {
    const results = await Promise.allSettled(scopes.map(async scope => {
      if (scope === "dashboard") {
        try {
          const value = await invoke<Dashboard>("dashboard_snapshot");
          setData(previous => retainSnapshot(previous, value));
          setConnected(true);
        } catch (error) { setConnected(false); throw error; }
      } else if (scope === "nodes") {
        const value = await invoke<NodeView[]>("list_node_views");
        setNodes(previous => retainSnapshot(previous, value));
      } else {
        const value = await invoke<AppSettings>("get_settings");
        setSettings(previous => retainSnapshot(previous, value));
      }
    }));
    const errors = results.flatMap(result => result.status === "rejected" ? [String(result.reason)] : []);
    if (errors.length) throw new Error(errors.join("；"));
  }), []);
  const refreshLive = useCallback(async () => {
    await Promise.all([refresh("dashboard"), refresh("nodes")]);
  }, [refresh]);
  return { data, nodes, settings, setSettings, connected, refresh, refreshLive };
}
