import { useEffect, useId, useState, type CSSProperties } from "react";
import { Api, errorText } from "./api";
import { CognitionEditor } from "./CognitionEditor";
import { Icon } from "./Icon";
import { Notice } from "./components";
import type { Cognition } from "./types";

const facets: { type: Cognition["type"]; label: string; hint: string }[] = [
  { type: "interest", label: "关注领域", hint: "最近想持续了解什么？" },
  { type: "preference", label: "阅读偏好", hint: "哪些材料更能帮助你思考？" },
  { type: "goal", label: "正在追求", hint: "有什么想推进的事？" },
  {
    type: "belief",
    label: "观点与判断",
    hint: "有哪些值得保留、也允许修正的判断？",
  },
  { type: "constraint", label: "现实条件", hint: "有哪些需要一起考虑的条件？" },
];

function point(index: number, radius: number) {
  const angle = ((-90 + index * 72) * Math.PI) / 180;
  return {
    x: 210 + Math.cos(angle) * radius,
    y: 162 + Math.sin(angle) * radius,
  };
}

export function PreferencePortrait({
  api,
  active,
  refreshKey,
  onAll,
  fixedInterests,
  onPreferences,
  onSource,
}: {
  api: Api;
  active: boolean;
  refreshKey: number;
  onAll: () => void;
  fixedInterests: string[];
  onPreferences: () => void;
  onSource: (id: string) => void;
}) {
  const [items, setItems] = useState<Cognition[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [reload, setReload] = useState(0);
  const [selected, setSelected] = useState<Cognition["type"]>("interest");
  const [editing, setEditing] = useState<Cognition | "new" | null>(null);
  const panelId = useId();
  const paintId = useId().replace(/:/g, "");
  useEffect(() => {
    if (!active) return;
    const controller = new AbortController();
    void api
      .request<{ items: Cognition[] }>(
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
  }, [api, active, refreshKey, reload]);
  const used = items.filter((item) => item.status === "active");
  const pending = items.filter((item) => item.status === "pending");
  const counts = facets.map(
    (facet) =>
      used.filter((item) => item.type === facet.type).length +
      (facet.type === "interest" ? fixedInterests.length : 0),
  );
  // Radius encodes record count, never confidence or a personality measurement.
  const points = counts.map((count, i) =>
    point(i, 54 + (56 * count) / (count + 2)),
  );
  const path =
    points.map((p, i) => `${i ? "L" : "M"} ${p.x} ${p.y}`).join(" ") + " Z";
  const entries = items.filter(
    (item) => item.type === selected && item.status !== "dismissed",
  );
  const facet = facets.find((item) => item.type === selected)!;
  return (
    <section
      className="preference-portrait"
      aria-labelledby={`${panelId}-title`}
    >
      <div className="portrait-heading">
        <div>
          <h1 id={`${panelId}-title`}>对我的了解</h1>
          <p>从你的表达和反馈中，慢慢更新。</p>
        </div>
        <button className="text-button" onClick={onAll}>
          全部记录 <Icon name="arrow" size={16} />
        </button>
      </div>
      {error && (
        <Notice error={error} onRetry={() => setReload((n) => n + 1)} />
      )}
      {editing && (
        <CognitionEditor
          key={editing === "new" ? `new-${selected}` : editing.id}
          api={api}
          item={editing === "new" ? undefined : editing}
          initialType={selected}
          onBack={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            setReload((n) => n + 1);
            setNotice("已更新，你的修改会用于之后的个性化研究。");
          }}
        />
      )}
      <>
        <div className="portrait-body" aria-busy={loading}>
          <div className="portrait-figure">
            <div
              className="portrait-orbit"
              role="group"
              aria-label="选择一种个人偏好"
            >
              <svg viewBox="0 0 420 326" aria-hidden="true">
                <defs>
                  <linearGradient id={paintId} x1="0" y1="0" x2="1" y2="1">
                    <stop offset="0%" stopColor="var(--portrait-warm)" />
                    <stop offset="100%" stopColor="var(--portrait-cool)" />
                  </linearGradient>
                </defs>
                <circle className="portrait-guide" cx="210" cy="162" r="113" />
                {facets.map((f, i) => {
                  const end = point(i, 113);
                  return (
                    <line
                      key={f.type}
                      className="portrait-spoke"
                      x1="210"
                      y1="162"
                      x2={end.x}
                      y2={end.y}
                    />
                  );
                })}
                <path
                  className="portrait-contour"
                  d={path}
                  style={
                    {
                      d: `path('${path}')`,
                      fill: `url(#${paintId})`,
                    } as CSSProperties
                  }
                />
                {points.map((p, i) => (
                  <circle
                    key={facets[i].type}
                    className={`portrait-node ${selected === facets[i].type ? "selected" : ""}`}
                    cx={p.x}
                    cy={p.y}
                    r={selected === facets[i].type ? 5 : 3.5}
                  />
                ))}
                <circle className="portrait-center" cx="210" cy="162" r="43" />
              </svg>
              <div className="portrait-self" aria-hidden="true">
                <span>我</span>
                <small>
                  {loading
                    ? "正在了解…"
                    : `${used.length + fixedInterests.length} 条记录`}
                </small>
              </div>
              {facets.map((f, i) => {
                const outstanding = pending.filter(
                  (item) => item.type === f.type,
                ).length;
                return (
                  <button
                    key={f.type}
                    className={`portrait-label facet-${i} ${selected === f.type ? "selected" : ""}`}
                    aria-pressed={selected === f.type}
                    aria-controls={panelId}
                    onClick={() => {
                      setSelected(f.type);
                      setNotice("");
                    }}
                  >
                    <strong>{f.label}</strong>
                    <small>
                      {loading ? "读取中" : `${counts[i]} 条已采用`}
                      {outstanding > 0 && (
                        <span className="portrait-pending">
                          {" "}
                          · {outstanding} 待确认
                        </span>
                      )}
                    </small>
                  </button>
                );
              })}
            </div>
            <p className="portrait-caption">
              轮廓随已采用的记录数量展开 · 点选方向可编辑
            </p>
          </div>
          <div className="portrait-details" id={panelId}>
            <div className="portrait-detail-heading">
              <h3>{facet.label}</h3>
              <button
                className="text-button"
                disabled={loading || !!error}
                onClick={() => setEditing("new")}
              >
                <Icon name="plus" size={15} />
                补充
              </button>
            </div>
            {selected === "interest" && fixedInterests.length > 0 && (
              <div className="portrait-subscriptions">
                <p>已订阅的领域</p>
                <div>
                  {fixedInterests.map((interest) => (
                    <span key={interest}>{interest}</span>
                  ))}
                </div>
                <button className="text-button" onClick={onPreferences}>
                  编辑订阅
                </button>
              </div>
            )}
            {loading ? (
              <p className="field-hint" role="status">
                正在读取你的偏好…
              </p>
            ) : !entries.length &&
              !(selected === "interest" && fixedInterests.length) ? (
              <p className="portrait-empty">
                {facet.hint}
                <br />
                你可以亲自补充，也可以等日常记录慢慢积累。
              </p>
            ) : (
              <ul className="portrait-entries">
                {entries.map((item) => (
                  <li key={item.id}>
                    <button
                      className="portrait-entry"
                      onClick={() => setEditing(item)}
                      aria-label={`编辑：${item.statement}`}
                    >
                      <span>
                        <small>
                          {item.origin === "manual"
                            ? "你填写的"
                            : item.origin === "explicit"
                              ? "来自明确表达"
                              : "AI 推测"}
                          <em>
                            {item.status === "pending" ? "待确认" : "已采用"}
                          </em>
                        </small>
                        <span>{item.statement}</span>
                      </span>
                      <Icon name="edit" size={15} />
                    </button>
                    {!!item.evidenceRefs?.length && (
                      <details className="portrait-evidence">
                        <summary>查看依据与原话</summary>
                        {item.inferenceReason && <p>{item.inferenceReason}</p>}
                        {item.evidenceRefs.map((evidence, index) => (
                          <div key={index}>
                            <blockquote>{evidence.quote}</blockquote>
                            {evidence.sourceId && (
                              <button
                                className="text-button"
                                onClick={() => onSource(evidence.sourceId!)}
                              >
                                查看原始记录 <Icon name="arrow" size={14} />
                              </button>
                            )}
                          </div>
                        ))}
                      </details>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
        {notice && (
          <p className="success-message" role="status">
            {notice}
          </p>
        )}
      </>
    </section>
  );
}
