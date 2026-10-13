import { useState } from "react";
import { Api, errorText, shortDate } from "./api";
import { Empty, Notice } from "./components";
import { Icon } from "./Icon";
import type { DailySummary, Report, Thought } from "./types";
import { ThoughtEditor } from "./ThoughtEditor";
import { useThoughtList } from "./useThoughtList";
import { ThoughtTimeline } from "./ThoughtTimeline";

export function Thoughts({
  api,
  thoughts: recentThoughts,
  reports,
  dailySummaries,
  timezone,
  selectedId,
  onSelect,
  onReport,
  onChanged: onParentChanged,
  onResearch,
  onDiscuss,
  onAdd,
  onSource,
  busy,
}: {
  api: Api;
  thoughts: Thought[];
  reports: Report[];
  dailySummaries: DailySummary[];
  timezone: string;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  onReport: (id: string) => void;
  onChanged: () => void;
  onResearch: (id: string) => Promise<void>;
  onDiscuss: (thought: Thought) => void;
  onAdd: () => void;
  onSource: (id: string) => void;
  busy: boolean;
}) {
  const page = useThoughtList(api, recentThoughts);
  const thoughts = page.items;
  const onChanged = () => {
    page.reload();
    onParentChanged();
  };
  const [error, setError] = useState("");
  const [pending, setPending] = useState("");
  const [showClosed, setShowClosed] = useState(false);
  const [editing, setEditing] = useState<Thought | "new" | null>(null);
  const items = thoughts.filter((t) =>
    showClosed
      ? t.status === "dismissed" || t.status === "resolved"
      : t.status !== "dismissed" && t.status !== "resolved",
  );
  const selected = thoughts.find((t) => t.id === selectedId);
  async function feedback(id: string, action: string) {
    setPending(id);
    setError("");
    try {
      await api.request(
        `/api/v1/thoughts/${encodeURIComponent(id)}/feedback`,
        "POST",
        { action },
      );
      onChanged();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setPending("");
    }
  }
  if (editing)
    return (
      <ThoughtEditor
        key={editing === "new" ? "new" : editing.id}
        api={api}
        thought={editing === "new" ? undefined : editing}
        onBack={() => setEditing(null)}
        onSaved={() => {
          setEditing(null);
          onChanged();
          onSelect(null);
        }}
      />
    );
  if (selected) {
    const related = reports.filter((r) => r.thoughtId === selected.id);
    return (
      <article className="thought-detail">
        <button
          className="text-button back-button"
          onClick={() => onSelect(null)}
        >
          <Icon name="back" size={18} />
          所有思考
        </button>
        {error && <Notice error={error} />}
        <h1>{selected.title}</h1>
        <div className="detail-meta">
          <span>{shortDate(selected.createdAt)}</span>
          <span>
            {selected.origin === "unverified_transcript"
              ? "来自对话 · 说话人未确认"
              : "来自你的记录"}
          </span>
          {selected.status === "kept" && <span>已保留</span>}
        </div>
        <p className="thought-question">
          {selected.analysisStatus === "ready"
            ? selected.question
            : "正在整理这段对话。"}
        </p>
        {selected.analysisStatus !== "ready" && (
          <p className="partial-note">
            {selected.analysisStatus === "model_unconfigured"
              ? "文字服务暂未就绪，恢复后会继续分析。"
              : selected.analysisStatus === "failed"
                ? "分析未完成，可以在「阅读」查看失败原因并重新处理。"
                : "正在等待分析，请稍后刷新。"}
          </p>
        )}
        <details className="original-record">
          <summary>
            <Icon name="file" size={16} />
            对话原文
            <Icon name="chevron" size={16} />
          </summary>
          <blockquote>{selected.quote}</blockquote>
        </details>
        {selected.analysisStatus === "ready" && (
          <>
            {selected.gaps?.length > 0 && (
              <section className="question-section">
                <h2>值得再追问</h2>
                <ul className="open-questions">
                  {selected.gaps.map((g, i) => (
                    <li key={i}>{g}</li>
                  ))}
                </ul>
              </section>
            )}
            {selected.assumptions?.length > 0 && (
              <details className="thought-details">
                <summary>
                  可能的假设 <span>AI 推断</span>
                </summary>
                <ul>
                  {selected.assumptions.map((a, i) => (
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
              disabled={busy || pending === selected.id}
              onClick={() => void onResearch(selected.id)}
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
              下一份研究会放在这里。也可以现在开始寻找证据。
            </p>
          )}
        </section>
        <div className="thought-actions">
          <button className="primary" onClick={() => onDiscuss(selected)}>
            <Icon name="chat" size={17} />
            聊一聊
          </button>
          <button className="text-button" onClick={() => setEditing(selected)}>
            编辑
          </button>
          <details className="more-actions">
            <summary>管理思考</summary>
            <div>
              {[
                ["keep", "保留这个问题"],
                ["resolved", "已经想清楚了"],
                ["dismiss", "暂时放下"],
              ].map(([action, label]) => (
                <button
                  key={action}
                  disabled={pending === selected.id}
                  onClick={() => void feedback(selected.id, action)}
                >
                  {label}
                </button>
              ))}
            </div>
          </details>
        </div>
      </article>
    );
  }
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>思考</h1>
          <p className="muted">按天整理话题，夜间准备研究材料。</p>
        </div>
        <button
          className="icon-button"
          onClick={() => setEditing("new")}
          aria-label="补充一个研究问题"
        >
          <Icon name="plus" size={22} />
        </button>
      </div>
      <div className="filter-line" role="group" aria-label="思考状态">
        <button
          aria-pressed={!showClosed}
          className={!showClosed ? "selected" : ""}
          onClick={() => setShowClosed(false)}
        >
          正在思考
        </button>
        <button
          aria-pressed={showClosed}
          className={showClosed ? "selected" : ""}
          onClick={() => setShowClosed(true)}
        >
          已收起
        </button>
      </div>
      {items.length === 0 ? (
        <Empty
          icon="thought"
          title={showClosed ? "暂时没有收起的思考" : "还没有话题"}
          action={
            !showClosed && (
              <button className="primary" onClick={onAdd}>
                记下想法
                <Icon name="arrow" size={17} />
              </button>
            )
          }
        >
          {showClosed
            ? "想清楚的问题、暂时放下的线索，都能在这里重新找到。"
            : "录音或导入对话后，会在这里按天整理话题和问题。"}
        </Empty>
      ) : (
        <ThoughtTimeline
          thoughts={items}
          days={dailySummaries}
          reports={reports}
          timezone={timezone}
          onSelect={onSelect}
          onReport={onReport}
        />
      )}
      {page.error && <Notice error={page.error} onRetry={page.reload} />}
      {page.next && (
        <button
          className="secondary"
          disabled={page.loading}
          onClick={() => void page.more()}
        >
          {page.loading ? "加载中…" : "查看更早的思考"}
        </button>
      )}
    </>
  );
}
