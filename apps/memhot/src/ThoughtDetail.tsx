import { shortDate } from "./api";
import { Notice } from "./components";
import { Icon } from "./Icon";
import type { Report, Thought } from "./types";

export function ThoughtDetail({
  thought,
  reports,
  busy,
  error,
  onBack,
  onDelete,
  onEdit,
  onArchive,
  onResolved,
  onReport,
  onSource,
  onResearch,
  onDiscuss,
}: {
  thought: Thought;
  reports: Report[];
  busy: boolean;
  error: string;
  onBack: () => void;
  onDelete: () => void;
  onEdit: () => void;
  onArchive: () => void;
  onResolved: () => void;
  onReport: (id: string) => void;
  onSource: (id: string) => void;
  onResearch: () => Promise<void>;
  onDiscuss: () => void;
}) {
  const related = reports.filter((r) => r.thoughtId === thought.id);
  const closed =
    thought.status === "dismissed" || thought.status === "resolved";
  return (
    <article className="thought-detail">
      <div className="thought-detail-toolbar">
        <button className="text-button" onClick={onBack}>
          <Icon name="back" size={18} />
          所有思考
        </button>
        <button
          className="text-button delete-thought-button"
          disabled={busy}
          onClick={onDelete}
        >
          <Icon name="trash" size={16} />
          删除
        </button>
      </div>
      {error && <Notice error={error} />}
      <h1>{thought.title}</h1>
      <div className="detail-meta">
        <span>{shortDate(thought.createdAt)}</span>
        <span>{thought.category || "未分类"}</span>
        <span>
          {closed
            ? "已归档"
            : thought.origin === "unverified_transcript"
              ? "来自对话 · 说话人未确认"
              : "来自你的记录"}
        </span>
      </div>
      <p className="thought-question">
        {thought.analysisStatus === "ready"
          ? thought.question
          : "正在整理这段对话。"}
      </p>
      {thought.analysisStatus !== "ready" && (
        <p className="partial-note">
          {thought.analysisStatus === "failed"
            ? "分析未完成，可以在「阅读」查看原因并重新处理。"
            : "整理完成后，问题和线索会出现在这里。"}
        </p>
      )}
      {!!thought.quote && (
        <details className="original-record">
          <summary>
            <Icon name="file" size={16} />
            对话原文
            <Icon name="chevron" size={16} />
          </summary>
          <blockquote>{thought.quote}</blockquote>
          {thought.sourceId && (
            <button
              className="text-button"
              onClick={() => onSource(thought.sourceId!)}
            >
              查看完整记录
              <Icon name="arrow" size={15} />
            </button>
          )}
        </details>
      )}
      {thought.analysisStatus === "ready" && (
        <>
          {!!thought.gaps?.length && (
            <section className="question-section">
              <h2>值得再追问</h2>
              <ul className="open-questions">
                {thought.gaps.map((g, i) => (
                  <li key={i}>{g}</li>
                ))}
              </ul>
            </section>
          )}
          {!!thought.assumptions?.length && (
            <details className="thought-details">
              <summary>
                可能的假设 <span>AI 推断</span>
              </summary>
              <ul>
                {thought.assumptions.map((a, i) => (
                  <li key={i}>{a}</li>
                ))}
              </ul>
            </details>
          )}
        </>
      )}
      <section className="question-section">
        <div className="section-heading">
          <h2>围绕这个问题的材料</h2>
          <button
            className="text-button"
            disabled={busy || closed}
            onClick={() => void onResearch()}
          >
            <Icon name="plus" size={16} />
            找些材料
          </button>
        </div>
        {related.length ? (
          related.map((r) => (
            <button
              className="related-reading"
              key={r.id}
              onClick={() => onReport(r.id)}
            >
              <Icon name="book" size={18} />
              <span>
                {r.title}
                <small>
                  {shortDate(r.createdAt)} ·{" "}
                  {r.sourceCount ?? r.sources?.length ?? 0} 份来源
                </small>
              </span>
              <Icon name="chevron" size={17} />
            </button>
          ))
        ) : (
          <p className="muted section-note">
            {closed
              ? "还没有研究材料，继续思考后可以寻找。"
              : "相关研究会放在这里，也可以现在开始寻找材料。"}
          </p>
        )}
      </section>
      <div className="thought-actions">
        <button className="primary" onClick={onDiscuss}>
          <Icon name="chat" size={17} />
          聊一聊
        </button>
        <button className="text-button" disabled={busy} onClick={onEdit}>
          <Icon name="edit" size={16} />
          编辑思考
        </button>
      </div>
      <section className="thought-organize">
        <div>
          <h2>{closed ? "这个问题已归档" : "先把这个问题收起来"}</h2>
          <p>
            {closed
              ? "重新开始研究时，再把它移回来。"
              : "归档后暂停研究，问题和材料仍然保留。"}
          </p>
        </div>
        <div className="inline-actions">
          <button className="secondary" disabled={busy} onClick={onArchive}>
            <Icon name={closed ? "refresh" : "archive"} size={16} />
            {closed ? "继续思考" : "归档"}
          </button>
          {!closed && (
            <button
              className="text-button"
              disabled={busy}
              onClick={onResolved}
            >
              已经想清楚了
            </button>
          )}
        </div>
      </section>
    </article>
  );
}
