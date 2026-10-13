import { useEffect, useRef, useState, type FormEvent } from "react";
import { Api, errorText, shortDate } from "./api";
import { Icon } from "./Icon";
import { Loading, Notice } from "./components";
import { ExternalLink } from "./ExternalLink";
import type { CatalogSource, FeedCandidate, FeedInput, OpmlPreview } from "./subscriptionData";
import { useBackAction } from "./useBackAction";
import { decodeCatalog, decodePreview, decodeOpml } from "./subscriptionProtocol";
import { backLayer } from "./backNavigation";

export function SubscriptionAdd({ api, mode, onAdded, onClose }: {
  api: Api; mode: "link" | "opml"; onAdded: () => void; onClose: () => void;
}) {
  const [url, setUrl] = useState("");
  const [category, setCategory] = useState("");
  const [catalog, setCatalog] = useState<CatalogSource[]>([]);
  const [catalogError, setCatalogError] = useState("");
  const [catalogId, setCatalogId] = useState<string>();
  const [candidates, setCandidates] = useState<FeedCandidate[]>([]);
  const [imported, setImported] = useState<OpmlPreview | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [warnings, setWarnings] = useState<string[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const lock = useRef(false);
  const controller = useRef<AbortController | null>(null);
  const uploadVersion = useRef(0);
  useBackAction(true, () => { if (!busy) onClose(); }, backLayer.editor + 1);
  useEffect(() => {
    const abort = new AbortController();
    if (mode === "link") void api.request<unknown>("/api/v1/subscriptions/catalog", "GET", undefined, abort.signal)
      .then(data => { if (!abort.signal.aborted) setCatalog(decodeCatalog(data)); })
      .catch(e => { if (!abort.signal.aborted) setCatalogError(errorText(e)); });
    return () => { abort.abort(); controller.current?.abort(); uploadVersion.current++; };
  }, [api, mode]);
  async function run(action: (signal: AbortSignal) => Promise<void>) {
    if (lock.current) return;
    lock.current = true; setBusy(true); setError("");
    const abort = new AbortController(); controller.current = abort;
    try { await action(abort.signal); }
    catch (e) { if (!abort.signal.aborted) setError(errorText(e)); }
    finally { lock.current = false; if (!abort.signal.aborted) setBusy(false); }
  }
  async function preview(input: { url: string } | { catalogId: string }) {
    setCandidates([]); setWarnings([]);
    await run(async signal => {
      const result = decodePreview(await api.request<unknown>("/api/v1/subscriptions/preview", "POST", input, signal));
      if (!signal.aborted) { setCandidates(result.candidates); setWarnings(result.warnings); }
    });
  }
  function submitLink(e: FormEvent) {
    e.preventDefault(); setCatalogId(undefined); void preview({ url: url.trim() });
  }
  async function filePreview(file?: File) {
    if (!file) return;
    setImported(null); setSelected(new Set()); setError("");
    if (file.size > 256 * 1024) { setError("文件太大，请导出不超过 256 KB 的 OPML 订阅列表。"); return; }
    const version = ++uploadVersion.current;
    await run(async signal => {
      const xml = await file.text();
      if (signal.aborted || version !== uploadVersion.current) return;
      const result = decodeOpml(await api.request<unknown>("/api/v1/subscriptions/opml/preview", "POST", { xml }, signal));
      if (!signal.aborted && version === uploadVersion.current) {
        setImported(result); setSelected(new Set(result.feeds.map(f => f.url)));
      }
    });
  }
  function subscribe(feed: FeedCandidate) {
    void run(async signal => {
      await api.request("/api/v1/subscriptions", "POST", catalogId ? { catalogId, category: category.trim() } : { url: feed.url, title: feed.title, category: category.trim() }, signal);
      if (!signal.aborted) onAdded();
    });
  }
  function importSelected() {
    void run(async signal => {
      const feeds: FeedInput[] = imported!.feeds.filter(f => selected.has(f.url));
      await api.request("/api/v1/subscriptions/import", "POST", { feeds }, signal);
      if (!signal.aborted) onAdded();
    });
  }
  return <section className="subscription-add" aria-label={mode === "link" ? "添加来源" : "导入订阅"} aria-busy={busy}>
    <div className="subscription-panel-heading"><h2>{mode === "link" ? "添加来源" : "导入已有订阅"}</h2><button className="icon-button" aria-label="关闭添加来源" disabled={busy} onClick={onClose}><Icon name="close" size={18} /></button></div>
    {mode === "link" ? <>
      <form onSubmit={submitLink} className="subscription-link-form">
        <label htmlFor="subscription-url">网站或 RSS 地址</label>
        <div><input id="subscription-url" type="url" required value={url} maxLength={2048} placeholder="https://…" onChange={e => { setUrl(e.target.value); setCandidates([]); setCatalogId(undefined); }} disabled={busy} />
          <button className="primary" disabled={busy || !url.trim()}>{busy ? "读取中…" : "查看内容"}</button></div>
      </form>
      {!candidates.length && !busy && <div className="subscription-catalog">
        <h3>也可以从这些来源开始</h3>
        {catalogError && <p className="field-hint">推荐来源暂时无法读取，仍可输入地址添加。</p>}
        {catalog.map(source => <button key={source.id} onClick={() => {
          setCatalogId(source.id); setCategory(source.category); setUrl(source.url); void preview({ catalogId: source.id });
        }}><span><strong>{source.title}</strong><small>{source.description}</small></span><Icon name="chevron" size={17} /></button>)}
      </div>}
      {!!candidates.length && <label className="subscription-category">放入分类<input value={category} onChange={e => setCategory(e.target.value)} maxLength={80} placeholder="例如：商业与投资（可不填）" disabled={busy} /></label>}
      {candidates.map(feed => <div key={feed.url} className="subscription-preview">
        <h3>{feed.title}</h3>
        <p className="meta">{feed.coverage === "unavailable" ? "暂时无法预览这个来源" : feed.coverage === "empty" ? "当前没有文章，订阅后继续接收更新" : feed.coverage === "title_only" ? "可获取标题与原文链接" : "可获取标题、摘要与原文链接"}</p>
        {feed.error ? <Notice error={feed.error.message} /> : <>
          <ol>{feed.items.map(item => <li key={item.url}><ExternalLink href={item.url}>{item.title}</ExternalLink>
            <span className="meta">{item.publishedAt ? shortDate(item.publishedAt) || "日期未确认" : "日期未确认"}</span>
            {item.summary && <p>{item.summary}</p>}
          </li>)}</ol>
          <button className="primary" disabled={busy} onClick={() => subscribe(feed)}><Icon name="plus" size={16} />订阅这个来源</button>
        </>}
      </div>)}
    </> : <>
      <p className="muted">从阅读器导出 OPML 文件，把订阅和分类带过来。</p>
      <label className="subscription-file">选择 OPML 文件<input type="file" accept=".opml,.xml,text/xml,application/xml" disabled={busy} onChange={e => { void filePreview(e.target.files?.[0]); e.currentTarget.value = ""; }} /></label>
      {imported && <div className="subscription-import">
        <p>{imported.feeds.length ? `找到 ${imported.feeds.length} 个来源，选择想带过来的订阅。` : "没有找到可导入的公开订阅地址。"}{imported.duplicateCount > 0 && ` 已合并 ${imported.duplicateCount} 个重复地址。`}</p>
        {imported.feeds.map(feed => <label key={feed.url} className="subscription-check"><input type="checkbox" checked={selected.has(feed.url)} disabled={busy} onChange={e => setSelected(old => { const next = new Set(old); if (e.target.checked) next.add(feed.url); else next.delete(feed.url); return next; })} /><span>{feed.title}<small>{feed.category || "未分类"}</small></span></label>)}
        {!!imported.rejected.length && <details><summary>{imported.rejected.length} 个条目无法导入</summary><ul>{imported.rejected.map((r, i) => <li key={i}>{r.title || "未命名来源"}：{r.reason}</li>)}</ul></details>}
        {!!imported.feeds.length && <button className="primary" disabled={busy || !selected.size} onClick={importSelected}>导入 {selected.size} 个来源</button>}
      </div>}
    </>}
    {busy && <Loading />}
    {warnings.map((w, i) => <p className="field-hint" key={i}>{w}</p>)}
    {error && <Notice error={error} />}
    <p className="subscription-footnote">目前支持公开 RSS 和 Atom。公众号、B 站、知乎的作者主页接入还在准备中。</p>
  </section>;
}
