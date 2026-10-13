import { useState, type FormEvent } from "react";
import { Api, errorText } from "./api";
import { Notice } from "./components";
import { Icon } from "./Icon";
import type { Thought } from "./types";
import { useBackAction } from "./useBackAction";
import { backLayer } from "./backNavigation";

export function ThoughtEditor({
  api,
  thought,
  onBack,
  onSaved,
}: {
  api: Api;
  thought?: Thought;
  onBack: () => void;
  onSaved: () => void;
}) {
  const [title, setTitle] = useState(thought?.title || "");
  const [question, setQuestion] = useState(thought?.question || "");
  const [summary, setSummary] = useState(thought?.summary || "");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState(false);
  useBackAction(true, () => {
    if (!busy) { if (confirm) setConfirm(false); else onBack(); }
  }, backLayer.editor);
  async function save(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api.request(
        thought
          ? `/api/v1/thoughts/${encodeURIComponent(thought.id)}`
          : "/api/v1/thoughts",
        thought ? "PUT" : "POST",
        {
          title: title.trim(),
          question: question.trim(),
          summary: summary.trim(),
          ...(thought ? { revision: thought.revision } : {}),
        },
      );
      onSaved();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  async function remove() {
    if (!thought) return;
    setBusy(true);
    setError("");
    try {
      await api.request(
        `/api/v1/thoughts/${encodeURIComponent(thought.id)}`,
        "DELETE",
      );
      onSaved();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="settings-page thought-editor">
      <button
        className="text-button back-button"
        onClick={onBack}
        disabled={busy}
      >
        <Icon name="back" size={18} />
        返回思考
      </button>
      <h1>{thought ? "把问题说得更准确" : "补充一个值得追问的问题"}</h1>
      <form onSubmit={save}>
        <label>
          这个方向叫什么
          <input
            autoFocus
            required
            maxLength={120}
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="例如：AI 工具真正的付费需求"
          />
        </label>
        <label>
          你想弄明白什么
          <textarea
            required
            rows={4}
            maxLength={1800}
            value={question}
            onChange={(e) => setQuestion(e.target.value)}
            placeholder="一个问题、一个假设、一个还没想清楚的方向，都可以。"
          />
        </label>
        <label>
          补充背景（可选）
          <textarea
            rows={3}
            maxLength={2000}
            value={summary}
            onChange={(e) => setSummary(e.target.value)}
          />
        </label>
        <p className="field-hint">
          保存后会进入夜间研究。你的修改会保留，之后的自动整理不会覆盖它。
        </p>
        {error && <Notice error={error} />}
        <button
          className="primary"
          disabled={busy || !title.trim() || !question.trim()}
        >
          {busy ? "处理中…" : "保存问题"}
        </button>
      </form>
      {thought &&
        (confirm ? (
          <div className="delete-confirm">
            <p>删除这个思考？它将停止研究，相关原文仍保留。</p>
            <button
              className="danger-text"
              disabled={busy}
              onClick={() => void remove()}
            >
              确认删除
            </button>
            <button disabled={busy} onClick={() => setConfirm(false)}>
              取消
            </button>
          </div>
        ) : (
          <button
            className="text-button danger-text"
            onClick={() => setConfirm(true)}
          >
            删除这个思考
          </button>
        ))}
    </section>
  );
}
