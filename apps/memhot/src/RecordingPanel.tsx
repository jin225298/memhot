import { useEffect, useState } from "react";
import { errorText } from "./api";
import { Notice } from "./components";
import { openBatterySettings } from "./native";
import { Icon } from "./Icon";
import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "./ui/accordion";
import type { CaptureStatus } from "./types";
import { TranscriptionSettings } from "./TranscriptionSettings";

export function RecordingPanel({
  capture,
  active,
  recordBusy,
  onRecord,
  onError,
}: {
  capture: CaptureStatus;
  active: boolean;
  recordBusy: boolean;
  onRecord: () => Promise<void>;
  onError: (message: string) => void;
}) {
  const recording = capture.state === "recording";
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    const started = performance.now();
    let timer: ReturnType<typeof setInterval> | undefined;
    const tick = () =>
      setElapsed((capture.elapsedMs || 0) + performance.now() - started);
    const resume = () => {
      clearInterval(timer);
      if (active && recording && document.visibilityState === "visible") {
        tick();
        timer = setInterval(tick, 1000);
      }
    };
    resume();
    document.addEventListener("visibilitychange", resume);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", resume);
    };
  }, [active, recording, capture.elapsedMs]);
  const seconds = Math.floor(elapsed / 1000);
  const duration = [
    Math.floor(seconds / 3600),
    Math.floor(seconds / 60) % 60,
    seconds % 60,
  ]
    .map((n) => String(n).padStart(2, "0"))
    .join(":");
  return (
    <div className="audio-panel">
      <section
        className={`recorder-focus ${recording ? "is-recording" : ""}`}
        aria-label="录音"
      >
        <div className="mic-stage">
          <button
            className="mic-button"
            disabled={recordBusy || !capture.supported}
            aria-label={recording ? "停止录音" : "开始录音"}
            aria-pressed={recording}
            aria-describedby="recorder-status"
            onClick={() => void onRecord()}
          >
            <Icon name={recording ? "stop" : "mic"} size={38} />
          </button>
        </div>
        <div className="recorder-status" id="recorder-status">
          <h2>
            {recordBusy
              ? "请稍候…"
              : recording
                ? "录音中"
                : capture.state === "paused"
                  ? "录音已暂停"
                  : "开始录音"}
          </h2>
          {recording && capture.elapsedMs !== undefined && (
            <span className="recording-duration">{duration}</span>
          )}
          <p>
            {recording
              ? "再次点击停止 · 锁屏后继续"
              : capture.supported
                ? "只转写有说话声的片段"
                : null}
          </p>
          {capture.queuedCount > 0 && <p>{capture.queuedCount} 段待同步</p>}
        </div>
      </section>
      {capture.error && <Notice error={capture.error} />}
      <Accordion type="single" collapsible className="transcription-disclosure">
        <AccordionItem value="recording-settings">
          <AccordionTrigger>
            <span>录音设置</span>{" "}
            <small>
              {capture.asrMode === "local" ? "本机转写" : "云端转写"}
              {capture.blockedCount
                ? ` · ${capture.blockedCount} 段待处理`
                : ""}
            </small>
          </AccordionTrigger>
          <AccordionContent>
            <TranscriptionSettings capture={capture} onError={onError} />
            <details className="recording-guide">
              <summary>后台录音与隐私</summary>
              <ol>
                <li>首次录音时允许麦克风和通知权限。</li>
                <li>
                  在电池设置中允许后台运行，锁屏后也能继续录音。重启手机后请重新开启。
                </li>
                <li>请在参与者知情时录音。不需要的内容可以随时删除。</li>
                <li>多人对话不会自动归为你的观点，需要你确认说话人。</li>
              </ol>
              {capture.supported && (
                <button
                  className="secondary"
                  onClick={() =>
                    void openBatterySettings().catch((e) =>
                      onError(errorText(e)),
                    )
                  }
                >
                  打开电池设置 <Icon name="external" size={16} />
                </button>
              )}
            </details>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </div>
  );
}
