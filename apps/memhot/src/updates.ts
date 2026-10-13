import { invoke, isTauri } from "@tauri-apps/api/core";
import { version } from "../package.json";
import { captureStatus, stopCapture } from "./native";

export interface UpdateRelease {
  schemaVersion: number;
  packageName: string;
  version: string;
  versionCode: number;
  minSdk: number;
  abi: string;
  url: string;
  size: number;
  sha256: string;
  notes: string[];
  publishedAt: string;
}
export interface UpdateStatus {
  supported: boolean;
  currentVersion: string;
  currentVersionCode: number;
  phase: "idle" | "available" | "downloading" | "ready" | "permission" | "installing" | "failed";
  release?: UpdateRelease | null;
  downloadedBytes: number;
  totalBytes: number;
  canInstall: boolean;
  error?: string | null;
}
export const browserUpdateStatus: UpdateStatus = {
  supported: false, currentVersion: version, currentVersionCode: 0,
  phase: "idle", downloadedBytes: 0, totalBytes: 0, canInstall: false,
};
export type UpdateCommand = "get_status" | "check_update" | "download_update" | "cancel_download" | "install_update";

export async function updateCommand(name: UpdateCommand): Promise<UpdateStatus> {
  if (!isTauri()) return browserUpdateStatus;
  try {
    return await invoke<UpdateStatus>(`plugin:memhot-updater|${name}`);
  } catch (error) {
    if (typeof error === "string" && error.trim()) throw new Error(error);
    throw error;
  }
}

/** Installation replaces the app process: never silently cut off a recording. */
export async function prepareUpdateInstall(stopRecording = false, isCurrent: () => boolean = () => true): Promise<
  { needsRecordingConsent: true } | { needsRecordingConsent: false; status: UpdateStatus }
> {
  const ensureCurrent = () => { if (!isCurrent()) throw new Error("已暂停安装，可稍后继续"); };
  const activeCapture = (state: string) => ["starting", "recording", "paused"].includes(state);
  ensureCurrent();
  const recording = await captureStatus();
  ensureCurrent();
  if (activeCapture(recording.state)) {
    if (!stopRecording) return { needsRecordingConsent: true };
    await stopCapture();
    ensureCurrent();
    const afterStop = await captureStatus();
    ensureCurrent();
    if (activeCapture(afterStop.state)) {
      throw new Error("录音还在结束，请稍后再更新");
    }
  }
  ensureCurrent();
  return { needsRecordingConsent: false, status: await updateCommand("install_update") };
}
