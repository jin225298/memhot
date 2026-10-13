import { useCallback, useEffect, useRef, useState } from "react";
import { Api, errorText } from "./api";
import { Empty, Loading, Notice } from "./components";
import { Icon } from "./Icon";
import { SubscriptionCard } from "./SubscriptionCard";
import { SubscriptionAdd } from "./SubscriptionAdd";
import { SubscriptionItems } from "./SubscriptionItems";
import { listSubscriptions, type Subscription } from "./subscriptionData";
import { useBackAction } from "./useBackAction";
import { backLayer } from "./backNavigation";

// Extends the established preferences surface: soft floating source cards, with
// preview/import inline. The source's last successful update is the primary proof.
export function Subscriptions({ api }: { api: Api }) {
  const [sources, setSources] = useState<Subscription[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [mode, setMode] = useState<"link" | "opml" | null>(null);
  const [filter, setFilter] = useState("");
  const [busy, setBusy] = useState(false);
  const [reading, setReading] = useState<Subscription | null>(null);
  const lock = useRef(false);
  const epoch = useRef(0);
  const listing = useRef<AbortController | null>(null);
  const active = useRef(true);
  const requests = useRef(new Set<AbortController>());
  const refresh = useCallback(async (initial = false) => {
    listing.current?.abort();
    const revision = ++epoch.current;
    const abort = new AbortController(); requests.current.add(abort); listing.current = abort;
    if (initial) setLoading(true);
    try {
      const next = await listSubscriptions(api, abort.signal);
      if (!abort.signal.aborted && revision === epoch.current) { setSources(next); setError(""); }
    } catch (e) { if (!abort.signal.aborted && revision === epoch.current) setError(errorText(e)); }
    finally { requests.current.delete(abort); if (listing.current === abort) listing.current = null; if (!abort.signal.aborted && revision === epoch.current) setLoading(false); }
  }, [api]);
  useEffect(() => {
    active.current = true; void refresh(true);
    return () => { active.current = false; epoch.current++; requests.current.forEach(c => c.abort()); requests.current.clear(); };
  }, [refresh]);
  const pending = sources.some(s => s.status === "collecting" || s.status === "pending");
  useEffect(() => {
    if (!pending) return;
    const timer = setInterval(() => { if (document.visibilityState === "visible" && !lock.current && !listing.current) void refresh(); }, 5000);
    return () => clearInterval(timer);
  }, [pending, refresh]);
  useBackAction(Boolean(mode) && !reading, () => setMode(null), backLayer.editor);
  async function action(id: string, kind: "refresh" | "update" | "delete", body?: unknown) {
    if (lock.current) return false;
    lock.current = true; setBusy(true); setError(""); setNotice("");
    const abort = new AbortController(); requests.current.add(abort);
    try {
      await api.request(`/api/v1/subscriptions/${encodeURIComponent(id)}${kind === "refresh" ? "/refresh" : ""}`, kind === "refresh" ? "POST" : kind === "delete" ? "DELETE" : "PUT", body, abort.signal);
      if (!active.current || abort.signal.aborted) return false;
      setNotice(kind === "refresh" ? "已安排更新，完成后会显示新内容数量。" : kind === "delete" ? "已取消订阅" : "已保存");
      await refresh(); return true;
    } catch (e) { if (!abort.signal.aborted) setError(errorText(e)); return false; }
    finally { requests.current.delete(abort); lock.current = false; if (active.current) setBusy(false); }
  }
  const visible = sources.filter(s => `${s.title} ${s.category} ${s.host}`.toLocaleLowerCase().includes(filter.toLocaleLowerCase().trim()));
  if (reading) return <SubscriptionItems api={api} source={reading} onBack={() => setReading(null)} />;
  return <div className="subscriptions">
    <p className="subscription-intro">把常读的来源放在一起，新的内容会成为日报和研究的材料。</p>
    {mode ? <SubscriptionAdd key={mode} api={api} mode={mode} onClose={() => setMode(null)} onAdded={() => {
      setMode(null); setNotice("订阅已添加，正在准备首次更新。"); void refresh();
    }} /> : <div className="subscription-toolbar">
      <button className="primary" disabled={busy} onClick={() => { setMode("link"); setNotice(""); }}><Icon name="plus" size={17} />添加来源</button>
      <button className="secondary" disabled={busy} onClick={() => { setMode("opml"); setNotice(""); }}><Icon name="upload" size={17} />导入订阅</button>
    </div>}
    {error && <Notice error={error} onRetry={() => void refresh()} />}
    {notice && <p role="status" className="success-message"><Icon name="check" size={16} />{notice}</p>}
    {loading ? <Loading /> : !mode && !sources.length ? <Empty icon="book" title="从你常读的来源开始">添加一个网站，或把已有的订阅列表导入进来。</Empty> : <>
      {!!sources.length && <div className="subscription-list-heading"><h2>已订阅 <span>{sources.length}</span></h2>
        <button className="text-button" disabled={busy} onClick={() => void refresh()} aria-label="刷新订阅状态"><Icon name="refresh" size={16} />刷新状态</button>
      </div>}
      {sources.length > 5 && <label className="subscription-search"><Icon name="search" size={18} /><input aria-label="搜索订阅" placeholder="搜索来源或分类" value={filter} onChange={e => setFilter(e.target.value)} /></label>}
      <div className="subscription-list">{visible.map(source => <SubscriptionCard key={source.id} source={source} busy={busy} onAction={action} onRead={() => setReading(source)} />)}</div>
      {!!sources.length && !visible.length && <p className="muted">没有匹配的订阅，试试其他名称或分类。</p>}
    </>}
    {!mode && !!sources.length && <p className="subscription-footnote">服务器会定时收取内容。立即更新只检查新文章，不会生成报告或消耗研究额度。</p>}
  </div>;
}
