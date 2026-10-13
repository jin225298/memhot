import { invoke, isTauri } from "@tauri-apps/api/core";
import type { CaptureStatus, Session } from "./types";
const browserStatus: CaptureStatus = {
  supported: false,
  state: "idle",
  queuedCount: 0,
  uploadedCount: 0,
};
async function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(`plugin:memhot-capture|${name}`, args);
  } catch (error) {
    // Rust Result errors arrive as strings, not JavaScript Error instances.
    // Normalize at the native boundary so the UI preserves actionable messages.
    if (typeof error === "string" && error.trim()) throw new Error(error);
    throw error;
  }
}
export async function loadSession(): Promise<Session | null> {
  if (isTauri()) {
    const s = await command<Session>("get_config");
    return s?.token ? s : null;
  }
  // Browser preview keeps credentials only for this tab, never in persistent localStorage.
  const raw = sessionStorage.getItem("memhot-session");
  if (!raw) return null;
  try {
    const s = JSON.parse(raw);
    return typeof s.token === "string" && typeof s.baseUrl === "string"
      ? s
      : null;
  } catch {
    return null;
  }
}
export async function saveSession(session: Session) {
  if (isTauri())
    await command("configure", {
      baseUrl: session.baseUrl,
      token: session.token,
      userId: session.userId || "",
    });
  else sessionStorage.setItem("memhot-session", JSON.stringify(session));
}
export async function captureStatus(): Promise<CaptureStatus> {
  return isTauri() ? command("status") : browserStatus;
}
export async function getLaunchTarget(): Promise<{ tab: "today" | null }> {
  return isTauri() ? command("get_launch_target") : { tab: null };
}
export async function startCapture() {
  if (!isTauri())
    throw new Error("请在手机应用中开启录音；也可以导入已有对话");
  await command("start");
}
export async function stopCapture() {
  if (isTauri()) await command("stop");
}
export async function clearPendingCapture() {
  if (isTauri()) await command("clear_pending");
}
export async function requestNotifications() {
  if (isTauri()) await command("request_notifications");
  else
    throw new Error("请在手机应用中开启通知；也可打开「阅读」查看");
}
export async function openBatterySettings() {
  if (isTauri()) await command("open_battery_settings");
}
export async function setAsrMode(mode: "cloud" | "local") {
  if (isTauri()) await command("set_asr_mode", { mode });
}
export async function downloadLocalModel() {
  if (isTauri()) await command("download_local_model");
}
export async function cancelLocalModelDownload() {
  if (isTauri()) await command("cancel_local_model_download");
}
export async function removeLocalModel() {
  if (isTauri()) await command("remove_local_model");
}
export async function retryPendingCapture(acceptDuplicateCharge: boolean) {
  if (isTauri()) await command("retry_pending", { acceptDuplicateCharge });
}
