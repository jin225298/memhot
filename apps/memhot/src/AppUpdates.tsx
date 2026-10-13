import { useEffect, useRef, useState } from "react";
import { errorText } from "./api";
import { Loading, Notice } from "./components";
import { Icon, Mark } from "./Icon";
import { backLayer } from "./backNavigation";
import { useBackAction } from "./useBackAction";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "./ui/dialog";
import { prepareUpdateInstall, updateCommand, type UpdateCommand, type UpdateStatus } from "./updates";

export function Updates() {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [checked, setChecked] = useState(false);
  const [recordingConfirm, setRecordingConfirm] = useState(false);
  const alive = useRef(false);
  const pending = useRef(false);
  const continueInstall = useRef(false);
  const revision = useRef(0);
  useBackAction(recordingConfirm, () => { if (!pending.current) setRecordingConfirm(false); }, backLayer.editor);

  // DownloadManager owns the transfer. Leaving this page only stops UI polling.
  useEffect(() => {
    alive.current = true;
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      if (!alive.current) return;
      if (!pending.current && document.visibilityState !== "hidden") {
        const startedAt = revision.current;
        try {
          const next = await updateCommand("get_status");
          if (alive.current && startedAt === revision.current) setStatus(next);
        } catch (e) {
          if (alive.current && startedAt === revision.current) { setError(errorText(e)); continueInstall.current = false; }
        }
      }
      if (alive.current) timer = setTimeout(() => void poll(), 1500);
    }
    void poll();
    return () => { alive.current = false; continueInstall.current = false; clearTimeout(timer); };
  }, []);

  async function action(command: UpdateCommand) {
    if (pending.current) return;
    revision.current++;
    pending.current = true;
    setBusy(true);
    setError("");
    if (command === "cancel_download") continueInstall.current = false;
    try {
      const next = await updateCommand(command);
      if (!alive.current) return;
      setStatus(next);
      if (command === "check_update") setChecked(true);
      if (command === "download_update") continueInstall.current = true;
    } catch (e) {
      continueInstall.current = false;
      if (alive.current) setError(errorText(e));
    } finally {
      pending.current = false;
      if (alive.current) setBusy(false);
    }
  }

  async function install(stopRecording = false) {
    if (pending.current) return;
    revision.current++;
    continueInstall.current = false;
    pending.current = true;
    setBusy(true);
    setError("");
    try {
      const result = await prepareUpdateInstall(stopRecording, () => alive.current && document.visibilityState !== "hidden");
      if (!alive.current) return;
      if (result.needsRecordingConsent) setRecordingConfirm(true);
      else { setRecordingConfirm(false); setStatus(result.status); }
    } catch (e) {
      if (alive.current) setError(errorText(e));
    } finally {
      pending.current = false;
      if (alive.current) setBusy(false);
    }
  }
  useEffect(() => {
    if (status?.phase === "ready" && continueInstall.current && !busy && document.visibilityState !== "hidden") {
      void install();
    }
    if (status?.phase === "failed") continueInstall.current = false;
  }, [status, busy]);

  if (!status) return error ? <Notice error={error} onRetry={() => void action("get_status")} /> : <Loading />;
  const release = status.release;
  const downloading = status.phase === "downloading";
  const installable = ["ready", "permission", "installing"].includes(status.phase);
  const total = status.totalBytes || release?.size || 1;
  const percent = Math.min(100, Math.round(status.downloadedBytes * 100 / total));
  const hint = !status.supported ? "当前平台暂不支持应用内下载安装更新。"
    : downloading ? "下载可以在后台继续。完成后将打开安装确认。"
    : status.phase === "permission" ? "请在系统设置中允许安装应用，返回后点「继续安装」。"
    : status.phase === "installing" ? "请完成系统安装确认。取消后也可以继续安装。"
    : installable ? "新版已下载，安装后重新打开即可。"
    : checked && !release ? "已是最新版本。" : "账号和资料会保留，安装前会校验文件与签名。";
  return (
    <div className="app-updates">
      <div className="update-version-card">
        <span className="update-mark"><Mark /></span>
        <div><h2>Memhot</h2><p>当前版本 {status.currentVersion}</p></div>
        {checked && !release && status.supported && <Icon name="check" size={22} />}
      </div>
      {(error || status.error) && <Notice error={error || status.error || ""} />}
      {release && (
        <section className="update-release-card" aria-label="新版内容">
          <div className="update-release-heading"><h2>{release.version}</h2><span>{(release.size / 1048576).toFixed(1)} MB</span></div>
          <ul>{release.notes.map((note, index) => <li key={index}>{note}</li>)}</ul>
          {downloading && <div className="update-progress">
            <progress aria-label="新版下载进度" max={total} value={status.downloadedBytes} />
            <p role="status">已下载 {percent}% <span>{(status.downloadedBytes / 1048576).toFixed(1)} / {(total / 1048576).toFixed(1)} MB</span></p>
          </div>}
        </section>
      )}
      <p className="update-hint" role="status">{busy ? "正在处理…" : hint}</p>
      {status.supported && <div className="update-actions">
        {downloading ? <button className="secondary" disabled={busy} onClick={() => void action("cancel_download")}>取消下载</button>
          : installable ? <button className="primary" disabled={busy} onClick={() => void install()}>继续安装 <Icon name="arrow" size={17} /></button>
          : release ? <button className="primary" disabled={busy} onClick={() => void action("download_update")}>{status.phase === "failed" ? "重新下载" : "立即更新"}<Icon name="arrow" size={17} /></button>
          : <button className="primary" disabled={busy} onClick={() => void action("check_update")}><Icon name="refresh" size={17} />检查更新</button>}
        {release && !downloading && <button className="text-button" disabled={busy} onClick={() => void action("check_update")}>重新检查</button>}
      </div>}
      <Dialog open={recordingConfirm} onOpenChange={(open) => { if (!busy) setRecordingConfirm(open); }}>
        <DialogContent closeDisabled={busy} closeLabel="关闭更新确认">
          <DialogTitle>结束这次录音并更新？</DialogTitle>
          <DialogDescription>更新会关闭应用。已录下的内容会保留，更新后可以重新开始录音。</DialogDescription>
          <div className="update-actions">
            <button className="primary" disabled={busy} onClick={() => void install(true)}>停止录音并更新</button>
            <button className="text-button" disabled={busy} onClick={() => setRecordingConfirm(false)}>稍后再说</button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
