import { useState } from "react";
import { Api, defaultServer, errorText } from "./api";
import { clearPendingCapture, saveSession, stopCapture } from "./native";
import { Notice } from "./components";
import { useBackAction } from "./useBackAction";
import { backLayer } from "./backNavigation";

export function DataControls({
  api,
  onCleared,
  onDisconnect,
  onSaved,
}: {
  api: Api;
  onCleared: () => void;
  onDisconnect: () => void;
  onSaved: () => void;
}) {
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState(false);
  useBackAction(confirm, () => { if (!busy) setConfirm(false); }, backLayer.editor);
  const [notice, setNotice] = useState("");
  async function clear() {
    setBusy(true);
    setError("");
    try {
      await stopCapture();
      await clearPendingCapture();
      await api.request("/api/v1/account/data", "DELETE");
      setConfirm(false);
      setNotice("个人内容已清理，账号与额度记录仍然保留。");
      onCleared();
      onSaved();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  async function logout() {
    setBusy(true);
    setError("");
    try {
      await stopCapture();
      await api.request("/api/v1/device", "DELETE");
      await saveSession({ baseUrl: defaultServer, token: "" });
      onDisconnect();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  }
  return (
    <section className="settings-section">
      <h2>哪些留在本机，哪些用于云端研究</h2>
      <dl className="storage-boundaries">
        <div>
          <dt>原始录音</dt>
          <dd>
            待处理片段留在本机应用私有目录，有存储上限。选择本地转写时，音频不上传；选择云端时，Memhot
            服务器只在转写过程中临时处理，不保存音频文件。
          </dd>
        </div>
        <div>
          <dt>对话文字与个人认知</dt>
          <dd>
            文字上传后按账号隔离保存，用于分话题整理、夜间研究和跨设备阅读。说话人不明确的对话，不会直接当作你的观点。
          </dd>
        </div>
        <div>
          <dt>网页材料</dt>
          <dd>抓取全文短期保留后清理，报告保留必要引用和原始链接。</dd>
        </div>
        <div>
          <dt>模型与账号</dt>
          <dd>
            离线模型保存在本机，可在录音设置中删除。密码只存哈希；模型服务凭据由服务器管理。第三方转写服务的数据处理受其自身政策约束。
          </dd>
        </div>
      </dl>
      <h2 className="account-section-title">管理这个空间</h2>
      <p className="field-hint">
        退出会停止录音并撤销当前会话；未同步的本地片段仍归属于这个账号，重新登录同一账号后可继续处理。
      </p>
      {error && <Notice error={error} />}
      <button
        className="secondary"
        disabled={busy}
        onClick={() => void logout()}
      >
        退出登录
      </button>
      {confirm ? (
        <div className="delete-confirm">
          <p>
            清理所有来源、话题、个人认知、研究、对话与待处理任务？账号和测试额度账本不受影响。
          </p>
          <p className="field-hint">
            历史备份副本在后续七次成功备份轮换后清理；需要立即清除时请联系管理员。
          </p>
          <button
            className="danger-text"
            disabled={busy}
            onClick={() => void clear()}
          >
            确认清理个人内容
          </button>
          <button disabled={busy} onClick={() => setConfirm(false)}>
            取消
          </button>
        </div>
      ) : (
        <div>
          <button
            className="text-button danger-text"
            onClick={() => setConfirm(true)}
          >
            清理全部个人内容
          </button>
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
