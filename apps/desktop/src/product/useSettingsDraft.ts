import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import type { Action, AppSettings, MutationResult } from "./types";

const portKeys = ["portStart", "portEnd", "cooldownHours"] as const;
const networkKeys = ["outboundMode", "outboundInterface"] as const;
export function useSettingsDraft(settings: AppSettings, onSettings: (value: AppSettings) => void, action: Action) {
  const [draft, setDraft] = useState(settings);
  const [saving, setSaving] = useState(false);
  const committed = useRef(settings);
  const pending = useRef(false);
  useEffect(() => {
    const previous = committed.current;
    committed.current = settings;
    setDraft(current => {
      const next = { ...settings };
      // Preserve only genuinely unsaved grouped inputs across external refreshes.
      for (const key of [...portKeys, ...networkKeys]) {
        if (current[key] !== previous[key]) Object.assign(next, { [key]: current[key] });
      }
      return next;
    });
  }, [settings]);
  const persist = async (keys: readonly (keyof AppSettings)[], patch: Partial<AppSettings>, label: string) => {
    if (pending.current) return false;
    pending.current = true;
    setSaving(true);
    const before = committed.current;
    const next = { ...before, ...patch };
    try {
      const result = await action(label, () => invoke<MutationResult<AppSettings>>("update_settings", { settings: next }), { reload: "none" });
      const saved = result.ok ? result.value?.value ?? next : before;
      committed.current = saved;
      setDraft(current => {
        const updated = { ...current };
        for (const key of keys) Object.assign(updated, { [key]: saved[key] });
        return updated;
      });
      onSettings(saved);
      return result.ok;
    } finally { pending.current = false; setSaving(false); }
  };
  const immediate = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) => void persist([key], { [key]: value }, "保存设置");
  const savePorts = () => void persist(portKeys, { portStart: draft.portStart, portEnd: draft.portEnd, cooldownHours: draft.cooldownHours }, "保存端口设置");
  const saveNetwork = () => void persist(networkKeys, { outboundMode: draft.outboundMode, outboundInterface: draft.outboundInterface }, "保存网络设置");
  return { draft, setDraft, saving, immediate, savePorts, saveNetwork };
}
