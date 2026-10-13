import { afterEach, expect, it, vi } from "vitest";
import { errorText } from "./api";
import { startCapture } from "./native";

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: vi.fn() }));
import { invoke } from "@tauri-apps/api/core";
afterEach(() => vi.resetAllMocks());

it("shows a native microphone rejection in the UI instead of a generic retry", async () => {
  vi.mocked(invoke).mockRejectedValue("麦克风权限被拒绝，未开始录音");
  let message = "";
  try { await startCapture(); } catch (error) { message = errorText(error); }
  expect(message).toBe("麦克风权限被拒绝，未开始录音");
});
