import { useState, type FormEvent } from "react";
import { shortDate } from "./api";
import { Icon } from "./Icon";
import { useBackAction } from "./useBackAction";
import { backLayer } from "./backNavigation";
import { subscriptionStatus, type Subscription } from "./subscriptionData";

export function SubscriptionCard({ source, busy, onAction, onRead }: {
  source: Subscription;
  busy: boolean;
  onRead: () => void;
  onAction: (id: string, action: "refresh" | "update" | "delete", body?: unknown) => Promise<boolean>;
}) {
  const [editing, setEditing] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [title, setTitle] = useState(source.title);
  const [category, setCategory] = useState(source.category);
  function close() { if (!busy) { setEditing(false); setRemoving(false); } }
  useBackAction(editing, close, backLayer.editor);
  async function save(e: FormEvent) {
    e.preventDefault();
    if (await onAction(source.id, "update", { title: title.trim(), category: category.trim() })) close();
  }
  return <article className="subscription-card" aria-label={source.title}>
    <div className="subscription-card-heading">
      <div><h2><button className="subscription-title" onClick={onRead}>{source.title}<Icon name="chevron" size={15} /></button></h2><p className="meta">{source.category || "未分类"} · {source.host}</p></div>
      <span className={`subscription-status is-${source.status}`}>{subscriptionStatus[source.status]}</span>
    </div>
    <p className="subscription-freshness">
      {source.lastSuccessAt ? `上次更新 ${shortDate(source.lastSuccessAt)} · 最近新增 ${source.lastNewCount} 篇` : "首次更新后，会显示这里收到了哪些新内容。"}
    </p>
    {source.error && source.enabled && <p className="subscription-error" role="status">{source.error}</p>}
    <div className="subscription-actions">
      <span className="meta">已收录 {source.itemCount} 篇</span>
      <button className="text-button" disabled={busy || !source.enabled || source.status === "collecting"} onClick={() => void onAction(source.id, "refresh")}>
        <Icon name="refresh" size={15} />{source.status === "collecting" ? "更新中" : "立即更新"}
      </button>
      <button className="text-button" aria-expanded={editing} disabled={busy} onClick={() => {
        setTitle(source.title); setCategory(source.category); setEditing(!editing); setRemoving(false);
      }}>管理</button>
    </div>
    {editing && <form className="subscription-edit" onSubmit={save}>
      <div className="subscription-fields">
        <label>名称<input required maxLength={120} value={title} onChange={e => setTitle(e.target.value)} disabled={busy} /></label>
        <label>分类<input maxLength={80} value={category} onChange={e => setCategory(e.target.value)} placeholder="例如：商业与投资" disabled={busy} /></label>
      </div>
      <div className="subscription-actions">
        <button type="submit" className="primary" disabled={busy || !title.trim()}>保存</button>
        <button type="button" className="text-button" disabled={busy} onClick={close}>取消</button>
        <button type="button" className="text-button" disabled={busy} onClick={() => void onAction(source.id, "update", { enabled: !source.enabled })}>{source.enabled ? "暂停订阅" : "恢复订阅"}</button>
      </div>
      <div className="subscription-remove">
        {removing ? <>
          <p>取消订阅后，不再收取这个来源。已有报告会保留。</p>
          <button type="button" className="text-button danger-text" disabled={busy} onClick={() => void onAction(source.id, "delete")}>确认取消订阅</button>
          <button type="button" className="text-button" disabled={busy} onClick={() => setRemoving(false)}>保留</button>
        </> : <button type="button" className="text-button danger-text" disabled={busy} onClick={() => setRemoving(true)}>取消这个订阅</button>}
      </div>
    </form>}
  </article>;
}
