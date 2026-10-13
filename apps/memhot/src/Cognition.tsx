import { useEffect, useState } from "react";
import { Api, errorText, shortDate } from "./api";
import { Empty, Loading, Notice } from "./components";
import { Icon } from "./Icon";
import type { Cognition as Item } from "./types";

import { CognitionEditor, cognitionLabels as labels } from "./CognitionEditor";
export function Cognition({
  api,
  onSource,
}: {
  api: Api;
  onSource: (id: string) => void;
}) {
  const [items, setItems] = useState<Item[]>([]);
  const [loading, setLoading] = useState(true);
  const [reload, setReload] = useState(0);
  const [filter, setFilter] = useState<Item["status"]>("active");
  const [editing, setEditing] = useState<Item | "new" | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState("");
  useEffect(() => {
    const controller = new AbortController();
    setLoading(true);
    void api
      .request<{ items: Item[] }>(
        "/api/v1/cognition",
        "GET",
        undefined,
        controller.signal,
      )
      .then((data) => {
        if (!controller.signal.aborted) {
          setItems(data.items);
          setError("");
        }
      })
      .catch((e) => {
        if (!controller.signal.aborted) setError(errorText(e));
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [api, reload]);
  async function change(item: Item, status: Item["status"]) {
    setBusy(item.id);
    setError("");
    try {
      await api.request(
        `/api/v1/cognition/${encodeURIComponent(item.id)}`,
        "PUT",
        { status, revision: item.revision },
      );
      setNotice(
        status === "active"
          ? "已确认，会用于之后的个性化研究。"
          : "已放下，不再作为个性化依据。",
      );
      setReload((n) => n + 1);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy("");
    }
  }
  const visible = items.filter((item) => item.status === filter);
  return (
    <section className="cognition-page">
      {editing && (
        <CognitionEditor
          key={editing === "new" ? "new" : editing.id}
          api={api}
          item={editing === "new" ? undefined : editing}
          onBack={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            setFilter("active");
            setReload((n) => n + 1);
            setNotice("已保存，你的修改优先于自动推断。");
          }}
        />
      )}
      <div className="page-heading">
        <div>
          <h1>对我的了解</h1>
          <p className="muted">查看、补充或纠正你的偏好。</p>
        </div>
        <button
          className="icon-button"
          aria-label="补充偏好"
          onClick={() => setEditing("new")}
        >
          <Icon name="plus" />
        </button>
      </div>
      <p className="editorial-note">
        你的明确表达和手动修改会影响之后的研究。说话人尚未确认的对话，只能留下待确认的理解。
      </p>
      <div className="section-tabs" role="group" aria-label="个人认知分类">
        {(
          [
            ["active", "用于个性化"],
            ["pending", "待你确认"],
            ["dismissed", "已放下"],
          ] as const
        ).map(([key, label]) => (
          <button
            key={key}
            className={filter === key ? "selected" : ""}
            aria-pressed={filter === key}
            onClick={() => setFilter(key)}
          >
            {label}
            <span>{items.filter((i) => i.status === key).length}</span>
          </button>
        ))}
      </div>
      {error && (
        <Notice error={error} onRetry={() => setReload((n) => n + 1)} />
      )}
      {notice && (
        <p role="status" className="success-message">
          {notice}
        </p>
      )}
      {loading && <Loading />}
      {!loading && !visible.length && (
        <Empty
          icon="person"
          title={
            filter === "active"
              ? "还没有偏好记录"
              : filter === "pending"
                ? "暂时没有需要确认的理解"
                : "暂时没有放下的认知"
          }
          action={
            filter === "active" && (
              <button className="primary" onClick={() => setEditing("new")}>
                添加偏好
              </button>
            )
          }
        >
          兴趣、目标、喜欢的材料类型，都可以写在这里。系统也会从你的表达与反馈中逐渐补充。
        </Empty>
      )}
      <div className="cognition-list">
        {visible.map((item) => (
          <article className="cognition-entry" key={item.id}>
            <div className="detail-meta">
              <span>{labels[item.type]}</span>
              <span>
                {item.origin === "manual"
                  ? "你填写的"
                  : item.origin === "explicit"
                    ? "来自明确表达"
                    : "AI 推断"}
              </span>
              <span>{shortDate(item.updatedAt)}</span>
            </div>
            <h2>{item.statement}</h2>
            {item.inferenceReason && (
              <p className="muted">{item.inferenceReason}</p>
            )}
            {!!item.evidenceRefs?.length && (
              <details className="cognition-evidence">
                <summary>
                  为什么这样理解 <Icon name="chevron" size={14} />
                </summary>
                {item.evidenceRefs.map((e, i) => (
                  <div key={i}>
                    <blockquote>{e.quote}</blockquote>
                    {e.sourceId && (
                      <button
                        className="text-button"
                        onClick={() => onSource(e.sourceId!)}
                      >
                        查看原文
                        <Icon name="arrow" size={15} />
                      </button>
                    )}
                  </div>
                ))}
              </details>
            )}
            <div className="inline-actions">
              {item.status !== "active" && (
                <button
                  className="text-button"
                  disabled={busy === item.id}
                  onClick={() => void change(item, "active")}
                >
                  <Icon name="check" size={16} />
                  {item.status === "pending" ? "理解得对" : "重新启用"}
                </button>
              )}
              <button
                className="text-button"
                disabled={busy === item.id}
                onClick={() => setEditing(item)}
              >
                修改
              </button>
              {item.status !== "dismissed" && (
                <button
                  className="text-button muted"
                  disabled={busy === item.id}
                  onClick={() => void change(item, "dismissed")}
                >
                  不适合我
                </button>
              )}
            </div>
          </article>
        ))}
      </div>
    </section>
  );
}
