import { useEffect, useState } from "react";
import { errorText, safeExternalUrl } from "./api";
import {
  cancelLocalModelDownload,
  captureStatus,
  downloadLocalModel,
  removeLocalModel,
  retryPendingCapture,
  setAsrMode,
} from "./native";
import { ExternalLink } from "./ExternalLink";
import { Notice } from "./components";
import type { CaptureStatus } from "./types";

export function TranscriptionSettings({
  capture,
  onError,
}: {
  capture: CaptureStatus;
  onError: (message: string) => void;
}) {
  const [state, setState] = useState(capture);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  const [retryConfirm, setRetryConfirm] = useState(false);
  useEffect(() => setState(capture), [capture]);
  useEffect(() => {
    if (!capture.supported || state.localModel?.state !== "downloading") return;
    let active = true;
    const timer = setInterval(() => {
      void captureStatus()
        .then((s) => {
          if (active) setState(s);
        })
        .catch(() => undefined);
    }, 3000);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [capture.supported, state.localModel?.state]);
  async function act(action: () => Promise<void>, message: string) {
    setBusy(true);
    setNotice("");
    try {
      await action();
      setState(await captureStatus());
      setNotice(message);
    } catch (e) {
      onError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  const model = state.localModel;
  const url = model?.licenseUrl && safeExternalUrl(model.licenseUrl);
  const downloading = model?.state === "downloading";
  return (
    <section className="transcription-settings">
      <h2>云端服务</h2>
      <p className="field-hint">
        默认由云端转写，直接开始录音即可。云端始终负责文字整理、个人记忆与夜间研究。
      </p>
      <p className="field-hint">
        Memhot
        服务器只在转写时临时接收音频，转完即清理；已整理的文字按你的保留设置保存。
      </p>
      {capture.supported && (
        <div className="local-model">
          <h3>
            本机转写 <span className="optional-label">可选安装</span>
          </h3>
          <p className="field-hint">
            想让原音留在手机，或减少云端转写额度时，再下载模型。无需安装也能完整使用云端服务。
          </p>
          <button
            type="button"
            role="switch"
            aria-checked={state.asrMode === "local"}
            className={`local-asr-switch ${state.asrMode === "local" ? "enabled" : ""}`}
            disabled={
              busy || (state.asrMode !== "local" && model?.state !== "ready")
            }
            onClick={() =>
              void act(
                () => setAsrMode(state.asrMode === "local" ? "cloud" : "local"),
                state.asrMode === "local"
                  ? "已恢复云端转写。"
                  : "已改用本机转写，仅同步文字。",
              )
            }
          >
            <span>
              使用本机转写
              <small>
                {state.asrMode === "local"
                  ? "仅上传文字，本机失败时提示处理"
                  : model?.state === "ready"
                    ? "模型已安装，可以随时切换"
                    : "下载完成后可开启"}
              </small>
            </span>
            <span className="switch-track" aria-hidden="true">
              <i />
            </span>
          </button>
          <p className="field-hint">SenseVoice Small · int8 · 中文等多语言</p>
          <p className="field-hint">
            下载约 156 MB，安装后约 230
            MB，支持中文。实际转写速度与耗电取决于设备；推荐先在 Wi-Fi 下下载。
            {url && (
              <>
                {" "}
                <ExternalLink href={url}>模型许可与归属</ExternalLink>
              </>
            )}
          </p>
          {model?.error && <Notice error={model.error} />}
          {downloading ? (
            <>
              <progress
                aria-label="离线模型下载进度"
                value={model.downloadedBytes}
                max={model.totalBytes || model.downloadBytes}
              />
              <p className="field-hint" role="status">
                已下载 {Math.round(model.downloadedBytes / 1048576)} /{" "}
                {Math.round(
                  (model.totalBytes || model.downloadBytes) / 1048576,
                )}{" "}
                MB
              </p>
              <button
                className="secondary"
                disabled={busy}
                onClick={() =>
                  void act(
                    cancelLocalModelDownload,
                    "已取消下载，未完成的文件会清理。",
                  )
                }
              >
                取消下载
              </button>
            </>
          ) : model?.state === "ready" ? (
            <div className="inline-actions">
              <span className="success-message">模型已就绪</span>
              <button
                className="text-button"
                disabled={busy || state.asrMode === "local"}
                onClick={() =>
                  void act(removeLocalModel, "离线模型已从本机删除。")
                }
              >
                删除本机模型
              </button>
            </div>
          ) : (
            <button
              className="secondary"
              disabled={busy}
              onClick={() =>
                void act(downloadLocalModel, "开始下载，可以继续使用其他功能。")
              }
            >
              {model?.state === "error"
                ? "重新下载模型"
                : "下载本机模型（可选）"}
            </button>
          )}
        </div>
      )}
      {!!state.noSpeechCount && (
        <p className="field-hint">
          已跳过 {state.noSpeechCount} 段无有效内容的转写，不进入话题分析。
        </p>
      )}
      {!!state.retryAfterMs && (
        <p className="field-hint">
          连续空结果后正在暂缓外发，约 {Math.ceil(state.retryAfterMs / 1000)}{" "}
          秒后恢复。
        </p>
      )}
      {(state.error || !!state.blockedCount) && state.queuedCount > 0 && (
        <div className="pending-recovery">
          {retryConfirm ? (
            <div className="delete-confirm">
              <p>
                如果上次云端请求结果不明，再次请求可能重复使用额度。确认重新处理本机待处理片段？
              </p>
              <button
                className="secondary"
                disabled={busy}
                onClick={() =>
                  void act(
                    () => retryPendingCapture(true),
                    "已重新安排处理。打开录音状态可查看进度。",
                  ).then(() => setRetryConfirm(false))
                }
              >
                确认重新处理
              </button>
              <button
                className="text-button"
                disabled={busy}
                onClick={() => setRetryConfirm(false)}
              >
                取消
              </button>
            </div>
          ) : (
            <button
              className="text-button"
              disabled={busy}
              onClick={() => {
                if (state.blockedCount) setRetryConfirm(true);
                else void act(() => retryPendingCapture(false), "已重新安排同步，将检查原片段的处理结果。");
              }}
            >
              重新处理待同步片段
            </button>
          )}
        </div>
      )}
      {notice && (
        <p className="success-message" role="status">
          {notice}
        </p>
      )}
    </section>
  );
}
