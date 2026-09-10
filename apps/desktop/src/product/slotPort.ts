import type { AppSettings, Slot } from "./types";

export function customPortError(input: string, settings: Pick<AppSettings, "portStart" | "portEnd">, slots: Slot[], selectedCount: number): string | null {
  const value = input.trim();
  if (!value) return null;
  if (!/^\d+$/.test(value)) return "请输入整数端口";
  const port = Number(value);
  if (!Number.isSafeInteger(port) || port < settings.portStart || port > settings.portEnd) return `端口必须在 ${settings.portStart}–${settings.portEnd} 范围内`;
  if (slots.some(slot => slot.port === port)) return "该端口已被其他 Slot 使用";
  if (selectedCount !== 1) return "指定端口时请选择且仅选择一个节点";
  return null;
}
