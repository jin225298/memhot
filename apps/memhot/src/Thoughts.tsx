import { useEffect, useState } from "react";
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
  status: "active",
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
  const [filters, setFilters] = useState<ThoughtFilters>(initialFilters);
  const [query, setQuery] = useState("");
  const [groupBy, setGroupBy] = useState<"date" | "category">("date");
  const [editing, setEditing] = useState<Thought | "new" | null>(null);
  const [removing, setRemoving] = useState<Thought | null>(null);
  const [opened, setOpened] = useState<Thought | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [pending, setPending] = useState("");
  const page = useThoughtList(api, recentThoughts, filters);
  useEffect(() => {
    const timer = setTimeout(
      () => setFilters((f) => ({ ...f, q: query.trim() })),
      300,
    );
    return () => clearTimeout(timer);
  }, [query]);
  const selected =
    page.items.find((t) => t.id === selectedId) ||
    (opened?.id === selectedId
      ? opened
      : recentThoughts.find((t) => t.id === selectedId));
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
    setPending(thought.id);
    setError("");
    try {
      await api.request(
        `/api/v1/thoughts/${encodeURIComponent(thought.id)}/feedback`,
        "POST",
        {
          action: resolved ? "resolved" : closed ? "keep" : "dismiss",
          revision: thought.revision,
        },
      );
      setNotice(
        closed
          ? "已移回正在思考。"
          : "已归档，可以在「已归档」中找到。不会因此减少类似推荐。",
      );
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
    setFilters((f) => ({ ...initialFilters, status: f.status }));
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
  if (selected)
    return (
      <>
        {deletion}
        <ThoughtDetail
          thought={selected}
          reports={reports}
          busy={busy || pending === selected.id}
          error={error}
          onBack={() => onSelect(null)}
          onDelete={() => setRemoving(selected)}
          onEdit={() => setEditing(selected)}
          onArchive={() => void archive(selected)}
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
          <p className="muted">把正在想的事，慢慢理清楚。</p>
        </div>
        <button className="secondary" onClick={() => setEditing("new")}>
          <Icon name="plus" size={18} />
          新增思考
        </button>
      </div>
      <div className="section-tabs" role="group" aria-label="思考状态">
        {(["active", "archived"] as const).map((status) => (
          <button
            key={status}
            aria-pressed={filters.status === status}
            className={filters.status === status ? "selected" : ""}
            onClick={() => setFilters((f) => ({ ...f, status }))}
          >
            {status === "active" ? "正在思考" : "已归档"}
            <span>{page.counts?.[status] || 0}</span>
          </button>
        ))}
      </div>
      <div className="thought-filters">
        <label className="archive-search thought-search">
          <Icon name="search" size={18} />
          <input
            type="search"
            maxLength={200}
            aria-label="搜索全部思考"
            placeholder="搜索问题、摘要和原话"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </label>
        <div className="thought-filter-row">
          <SelectField
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
          />
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
          title={
            filtering
              ? "没有找到这些思考"
              : filters.status === "archived"
                ? "还没有归档的思考"
                : "记下你想探索的问题"
          }
          action={
            <button
              className="secondary"
              onClick={
                filtering
                  ? reset
                  : filters.status === "archived"
                    ? () => setFilters(initialFilters)
                    : onAdd
              }
            >
              {filtering
                ? "清除筛选"
                : filters.status === "archived"
                  ? "回到正在思考"
                  : "记下想法"}
            </button>
          }
        >
          {filtering
            ? "试试其他关键词，搜索也会查找以前的记录。"
            : filters.status === "archived"
              ? "暂时放下或已经想清楚的问题，可以收在这里。"
              : "录音、导入对话，也可以直接新增思考。"}
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
            onArchive={(t) => void archive(t)}
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
