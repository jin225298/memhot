import { useEffect, useRef, useState } from "react";
import { Api, errorText } from "./api";
import { Empty, Loading, Notice } from "./components";
import { Icon } from "./Icon";
import type { DailySummary, Report, Thought } from "./types";
import { ThoughtEditor } from "./ThoughtEditor";
import { useThoughtList, type ThoughtFilters } from "./useThoughtList";
import { ThoughtTimeline } from "./ThoughtTimeline";
import { ThoughtDetail } from "./ThoughtDetail";
import { ThoughtDelete } from "./ThoughtDelete";
import { SelectField } from "./ui/select";

const initialFilters: ThoughtFilters = {
  scope: "today",
  status: "all",
  q: "",
  period: "all",
  category: "",
  date: "",
};
export function Thoughts({
  api,
  thoughts: recentThoughts,
  reports,
  timezone,
  selectedId,
  onSelect,
  onReport,
  onChanged: onParentChanged,
  onResearch,
  onDiscuss,
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
  const [filters, setFilters] = useState<ThoughtFilters>(initialFilters);
  const [query, setQuery] = useState("");
  const [groupBy, setGroupBy] = useState<"date" | "category">("date");
  const [editing, setEditing] = useState<Thought | "new" | null>(null);
  const [removing, setRemoving] = useState<Thought | null>(null);
  const [opened, setOpened] = useState<Thought | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [pending, setPending] = useState("");
  const page = useThoughtList(api, recentThoughts, filters, timezone);
  const [detailError, setDetailError] = useState("");
  const [detailRetry, setDetailRetry] = useState(0);
  const detailRequest = useRef<AbortController | null>(null);
  useEffect(() => {
    const timer = setTimeout(
      () => setFilters((f) => ({ ...f, q: query.trim() })),
      300,
    );
    return () => clearTimeout(timer);
  }, [query]);
  const selected =
    (opened?.id === selectedId ? opened : null) ||
    page.items.find((t) => t.id === selectedId) ||
    recentThoughts.find((t) => t.id === selectedId);
  useEffect(() => {
    if (!selectedId) return;
    const controller = new AbortController();
    detailRequest.current = controller;
    setDetailError("");
    api.request<Thought>(`/api/v1/thoughts/${encodeURIComponent(selectedId)}`, "GET", undefined, controller.signal)
      .then((thought) => { if (!controller.signal.aborted) setOpened(thought); })
      .catch((e) => { if (!controller.signal.aborted) setDetailError(errorText(e)); });
    return () => controller.abort();
  }, [api, selectedId, page.currentDay, detailRetry]);
  function switchScope(scope: "today" | "history") {
    setQuery("");
    setFilters({ ...initialFilters, scope });
    setError("");
  }
  async function addToday(thought: Thought) {
    if (pending) return;
    detailRequest.current?.abort();
    setPending(thought.id);
    setError("");
    try {
      const updated = await api.request<Thought>(
        `/api/v1/thoughts/${encodeURIComponent(thought.id)}/today`, "POST",
        { revision: thought.revision },
      );
      setOpened(updated);
      setNotice("已加入今天，原来的内容和研究材料仍然保留。");
      changed();
    } catch (e) { setError(errorText(e)); }
    finally { setPending(""); }
  }
  function changed() {
    page.reload();
    onParentChanged();
  }
  function select(id: string) {
    setOpened(page.items.find((t) => t.id === id) || null);
    onSelect(id);
  }
  async function archive(thought: Thought, resolved = false) {
    if (pending) return;
    const closed =
      thought.status === "dismissed" || thought.status === "resolved";
    if (closed) { await addToday(thought); return; }
    detailRequest.current?.abort();
    setPending(thought.id);
    setError("");
    try {
      await api.request(
        `/api/v1/thoughts/${encodeURIComponent(thought.id)}/feedback`,
        "POST",
        {
          action: resolved ? "resolved" : "dismiss",
          revision: thought.revision,
        },
      );
      setNotice("已收进历史，问题和研究材料仍然保留。");
      onSelect(null);
      changed();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setPending("");
    }
  }
  const filtering = Boolean(
    filters.q || filters.category || filters.date || filters.period !== "all",
  );
  function reset() {
    setQuery("");
    setFilters((f) => ({ ...initialFilters, scope: f.scope }));
  }
  const deletion = removing && (
    <ThoughtDelete
      key={removing.id}
      api={api}
      thought={removing}
      onClose={() => setRemoving(null)}
      onDeleted={(less) => {
        setRemoving(null);
        setOpened(null);
        onSelect(null);
        setNotice(
          less
            ? "已删除，并记下少推荐类似问题的偏好。可在「个人偏好」修改。"
            : "已删除思考，原始记录仍保留。",
        );
        changed();
      }}
    />
  );
  if (editing)
    return (
      <ThoughtEditor
        key={editing === "new" ? "new" : editing.id}
        api={api}
        thought={editing === "new" ? undefined : editing}
        onBack={() => setEditing(null)}
        onSaved={() => {
          setNotice(
            editing === "new"
              ? "已添加思考，也记下了你想探索的方向。可在「个人偏好」修改。"
              : "已保存修改。",
          );
          setEditing(null);
          changed();
          onSelect(null);
        }}
      />
    );
  if (selectedId && !selected)
    return <>
      <button className="text-button" onClick={() => onSelect(null)}><Icon name="back" size={18} />返回思考</button>
      {detailError ? <Notice error={detailError} onRetry={() => setDetailRetry((n) => n + 1)} /> : <Loading />}
    </>;
  if (selected)
    return (
      <>
        {deletion}
        {notice && <p className="success-message" role="status">{notice}</p>}
        <ThoughtDetail
          thought={selected}
          reports={reports}
          busy={busy || pending === selected.id}
          error={error || detailError}
          onBack={() => onSelect(null)}
          onDelete={() => setRemoving(selected)}
          onEdit={() => setEditing(selected)}
          onArchive={() => void archive(selected)}
          onAddToday={() => void addToday(selected)}
          onResolved={() => void archive(selected, true)}
          onReport={onReport}
          onSource={onSource}
          onResearch={() => onResearch(selected.id)}
          onDiscuss={() => onDiscuss(selected)}
        />
      </>
    );
  return (
    <>
      {deletion}
      <div className="page-heading">
        <div>
          <h1>思考</h1>
          <p className="muted">今天想什么，就从这里开始。</p>
        </div>
        <button className="secondary" onClick={() => setEditing("new")}>
          <Icon name="plus" size={18} />
          新增思考
        </button>
      </div>
      <div className="section-tabs" role="group" aria-label="思考日期">
        {(["today", "history"] as const).map((scope) => (
          <button
            key={scope}
            aria-pressed={filters.scope === scope}
            className={filters.scope === scope ? "selected" : ""}
            onClick={() => switchScope(scope)}
          >
            {scope === "today" ? "今天" : "历史"}
            {page.counts?.[scope] !== undefined && <span>{page.counts[scope]}</span>}
          </button>
        ))}
      </div>
      <div className="thought-filters">
        <label className="archive-search thought-search">
          <Icon name="search" size={18} />
          <input
            type="search"
            maxLength={200}
            aria-label={filters.scope === "today" ? "搜索今天的思考" : "搜索历史思考"}
            placeholder="搜索问题、摘要和原话"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </label>
        <div className="thought-filter-row">
          {filters.scope === "history" && <SelectField
            label="思考时间"
            value={filters.date ? `date:${filters.date}` : filters.period}
            onValueChange={(v) =>
              setFilters((f) => ({
                ...f,
                date: v.startsWith("date:") ? v.slice(5) : "",
                period: v.startsWith("date:") ? "all" : v,
              }))
            }
            options={[
              { value: "all", label: "全部时间" },
              { value: "week", label: "近一周" },
              { value: "month", label: "近一月" },
              ...(page.dates || []).map((d) => ({
                value: `date:${d}`,
                label: d.replaceAll("-", "."),
              })),
            ]}
          />}
          <SelectField
            label="思考分类"
            value={filters.category ? `category:${filters.category}` : "all"}
            onValueChange={(v) =>
              setFilters((f) => ({
                ...f,
                category: v === "all" ? "" : v.slice(9),
              }))
            }
            options={[
              { value: "all", label: "全部分类" },
              ...(page.categories || []).map((c) => ({
                value: `category:${c}`,
                label: c,
              })),
            ]}
          />
          {filtering && (
            <button className="text-button" onClick={reset}>
              清除筛选
            </button>
          )}
        </div>
      </div>
      {error && <Notice error={error} />}
      {notice && (
        <p className="success-message" role="status">
          {notice}
        </p>
      )}
      {page.error && <Notice error={page.error} onRetry={page.reload} />}
      <div className="collection-toolbar">
        <span aria-live="polite">
          {page.ready ? `${page.total} 个思考` : "正在查找…"}
        </span>
        <div className="group-switch" role="group" aria-label="归档分组">
          <button
            aria-pressed={groupBy === "date"}
            onClick={() => setGroupBy("date")}
          >
            按日期
          </button>
          <button
            aria-pressed={groupBy === "category"}
            onClick={() => setGroupBy("category")}
          >
            按分类
          </button>
        </div>
      </div>
      {!page.ready && !page.error ? (
        <Loading />
      ) : page.ready && page.items.length === 0 ? (
        <Empty
          icon="thought"
          title={filtering ? "没有找到这些思考" : filters.scope === "today" ? "今天还没有思考" : "还没有历史思考"}
          action={filtering ? (
            <button className="secondary" onClick={reset}>清除筛选</button>
          ) : (
            <div className="inline-actions thought-empty-actions">
              <button className="secondary" onClick={() => setEditing("new")}>新增思考</button>
              <button className="text-button" onClick={() => switchScope(filters.scope === "today" ? "history" : "today")}>
                {filters.scope === "today" ? "查看历史" : "回到今天"}
              </button>
            </div>
          )}
        >
          {filtering ? "试试其他关键词或筛选条件。" : filters.scope === "today"
            ? "记下新的想法，或从历史中选一个加入今天。"
            : "以前的思考会保留在这里。想继续时，点「加入今天」。"}
        </Empty>
      ) : (
        page.items.length > 0 && (
          <ThoughtTimeline
            key={`${JSON.stringify(filters)}:${groupBy}`}
            thoughts={page.items}
            reports={reports}
            timezone={timezone}
            groupBy={groupBy}
            expanded={Boolean(filters.q || filters.date || filters.category)}
            onSelect={select}
            scope={filters.scope}
            onArchive={(t) => void archive(t)}
            onAddToday={(t) => void addToday(t)}
            onDelete={setRemoving}
            pending={pending}
          />
        )
      )}
      {page.next && (
        <button
          className="secondary archive-more"
          disabled={page.loading}
          onClick={() => void page.more()}
        >
          {page.loading ? "加载中…" : "查看更早的思考"}
        </button>
      )}
    </>
  );
}
