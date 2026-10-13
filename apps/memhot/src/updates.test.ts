import { beforeEach, expect, it, vi } from "vitest";
import { captureStatus, stopCapture } from "./native";
import { invoke } from "@tauri-apps/api/core";
import { prepareUpdateInstall, updateCommand } from "./updates";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
vi.mock("./native", () => ({ captureStatus: vi.fn(), stopCapture: vi.fn() }));
const recording = { supported: true, state: "recording" as const, queuedCount: 0, uploadedCount: 0 };
beforeEach(() => vi.resetAllMocks());

it("requires consent before an update interrupts a recording", async () => {
  vi.mocked(captureStatus).mockResolvedValue(recording);
  expect(await prepareUpdateInstall()).toEqual({ needsRecordingConsent: true });
  expect(stopCapture).not.toHaveBeenCalled();
  expect(invoke).not.toHaveBeenCalled();
});
it("waits for recording to stop before opening the installer", async () => {
  vi.mocked(captureStatus).mockResolvedValueOnce(recording)
    .mockResolvedValueOnce({ ...recording, state: "idle" });
  vi.mocked(invoke).mockImplementation(async () => {
    expect(stopCapture).toHaveBeenCalledOnce();
    expect(captureStatus).toHaveBeenCalledTimes(2);
    return { phase: "installing" };
  });
  expect(await prepareUpdateInstall(true)).toEqual({ needsRecordingConsent: false, status: { phase: "installing" } });
});
it("does not install when recording fails to stop", async () => {
  vi.mocked(captureStatus).mockResolvedValue(recording);
  await expect(prepareUpdateInstall(true)).rejects.toThrow("录音还在结束");
  expect(invoke).not.toHaveBeenCalled();
});
it("does not install if native recording status is unavailable", async () => {
  vi.mocked(captureStatus).mockRejectedValue(new Error("status unavailable"));
  await expect(prepareUpdateInstall()).rejects.toThrow();
  expect(invoke).not.toHaveBeenCalled();
});
it("protects a recording that is still starting", async () => {
  vi.mocked(captureStatus).mockResolvedValue({ ...recording, state: "starting" } as unknown as typeof recording);
  expect(await prepareUpdateInstall()).toEqual({ needsRecordingConsent: true });
  expect(invoke).not.toHaveBeenCalled();
});
it("does not stop or install after leaving during a status request", async () => {
  let active = true;
  vi.mocked(captureStatus).mockImplementation(async () => { active = false; return recording; });
  await expect(prepareUpdateInstall(true, () => active)).rejects.toThrow("已暂停安装");
  expect(stopCapture).not.toHaveBeenCalled();
  expect(invoke).not.toHaveBeenCalled();
});
it("does not install after leaving while stopping a recording", async () => {
  let active = true;
  vi.mocked(captureStatus).mockResolvedValue(recording);
  vi.mocked(stopCapture).mockImplementation(async () => { active = false; });
  await expect(prepareUpdateInstall(true, () => active)).rejects.toThrow("已暂停安装");
  expect(invoke).not.toHaveBeenCalled();
});
it("preserves actionable native update errors", async () => {
  vi.mocked(invoke).mockRejectedValue("安装包校验失败，请重新下载");
  await expect(updateCommand("install_update")).rejects.toThrow("安装包校验失败，请重新下载");
});
