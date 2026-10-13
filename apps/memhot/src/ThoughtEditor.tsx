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
  const [category, setCategory] = useState(thought?.category || "");
  useBackAction(
    true,
    () => {
      if (!busy) onBack();
    },
    backLayer.editor,
  );
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
          category: category.trim(),
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
      <h1>{thought ? "编辑思考" : "新增思考"}</h1>
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
        <label>
          分类（可选）
          <input
            maxLength={40}
            value={category}
            onChange={(e) => setCategory(e.target.value)}
            placeholder="例如：产品、工作、生活"
          />
        </label>
        <p className="field-hint">
          保存后会安排研究，并记下你想探索的方向。对应偏好可以在「个人偏好」修改。
        </p>
        {error && <Notice error={error} />}
        <button
          className="primary"
          disabled={busy || !title.trim() || !question.trim()}
        >
          {busy ? "处理中…" : "保存问题"}
        </button>
      </form>
    </section>
  );
}
