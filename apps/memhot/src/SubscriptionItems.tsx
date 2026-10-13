import { useEffect, useState } from "react";
import { Api, errorText, shortDate } from "./api";
import { Loading, Notice } from "./components";
import { ExternalLink } from "./ExternalLink";
import { Icon } from "./Icon";
import { useBackAction } from "./useBackAction";
import { backLayer } from "./backNavigation";
import type { Subscription } from "./subscriptionData";

import { decodeItems, type SubscriptionItem } from "./subscriptionProtocol";
export function SubscriptionItems({ api, source, onBack }: { api: Api; source: Subscription; onBack: () => void }) {
  const [items, setItems] = useState<SubscriptionItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  useBackAction(true, onBack, backLayer.detail);
  useEffect(() => {
    const controller = new AbortController(); setLoading(true); setError("");
    void api.request<unknown>(`/api/v1/subscriptions/${encodeURIComponent(source.id)}/items`, "GET", undefined, controller.signal)
      .then(data => { if (!controller.signal.aborted) setItems(decodeItems(data)); })
      .catch(e => { if (!controller.signal.aborted) setError(errorText(e)); })
      .finally(() => { if (!controller.signal.aborted) setLoading(false); });
    return () => controller.abort();
  }, [api, source.id, retry]);
  return <section className="subscription-items">
    <button className="text-button back-button" onClick={onBack}><Icon name="back" size={17} />我的订阅</button>
    <h2>{source.title}</h2><p className="meta">{source.host} · 最近收录的内容</p>
    {loading ? <Loading /> : error ? <Notice error={error} onRetry={() => setRetry(n => n + 1)} /> : !items.length ? <p className="subscription-freshness">暂时没有收到文章。首次更新完成后会显示在这里。</p> : <ol className="subscription-articles">
      {items.map(item => <li key={item.id}><span className="meta">{item.publishedAt ? shortDate(item.publishedAt) || "日期未确认" : "日期未确认"}</span><h3><ExternalLink href={item.url}>{item.title}</ExternalLink></h3>{item.excerpt && <p>{item.excerpt}</p>}</li>)}
    </ol>}
    {!!items.length && <p className="subscription-footnote">最多显示最近 100 篇。这里是来源提供的标题与摘要，点击标题阅读原文。</p>}
  </section>;
}
