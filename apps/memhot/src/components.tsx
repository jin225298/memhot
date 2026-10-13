import type { ReactNode } from "react";
import { Icon, type IconName } from "./Icon";
import { safeExternalUrl, shortDate } from "./api";
import { ExternalLink } from "./ExternalLink";
import type { Evidence } from "./types";
export function Empty({
  icon,
  title,
  children,
  action,
}: {
  icon: IconName;
  title: string;
  children: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      <div className="empty-icon">
        <Icon name={icon} size={28} />
      </div>
      <h2>{title}</h2>
      <p>{children}</p>
      {action}
    </div>
  );
}
export function Notice({
  error,
  onRetry,
}: {
  error: string;
  onRetry?: () => void;
}) {
  return (
    <div className="notice" role="alert">
      <span>{error}</span>
      {onRetry && (
        <button className="text-button" onClick={onRetry}>
          重试
        </button>
      )}
    </div>
  );
}
export function Loading() {
  return (
    <div className="skeleton" role="status" aria-label="正在加载">
      <div />
      <div />
      <div />
      <span className="sr-only">正在加载…</span>
    </div>
  );
}
export function EvidenceList({
  evidence,
  onRead,
}: {
  evidence: Evidence[];
  onRead?: (id: string) => void;
}) {
  return (
    <div className="evidence-list">
      {evidence.map((item, index) => (
        <details key={item.id} className="evidence">
          <summary>
            <span className="source-index">{index + 1}</span>
            <span>
              <strong>{item.title}</strong>
              <small>
                {item.publisher || "原始来源"} ·{" "}
                {item.stance === "counter"
                  ? "不同观点"
                  : item.stance === "support"
                    ? "支持材料"
                    : "背景材料"}
                {item.retrievalKind === "metadata_or_abstract" &&
                  " · 仅摘要或元数据"}
              </small>
            </span>
            <Icon name="chevron" size={16} />
          </summary>
          <div className="evidence-content">
            <blockquote>
              {item.excerpt || "此来源未保存可核验摘录，请阅读原文。"}
            </blockquote>
            <p className="meta">
              {item.publishedAt
                ? `发布于 ${shortDate(item.publishedAt)} · `
                : "发布日期未确认 · "}
              检索于 {shortDate(item.retrievedAt)}
            </p>
            {onRead && (
              <button className="text-button" onClick={() => onRead(item.id)}>
                查看资料 <Icon name="book" size={15} />
              </button>
            )}
            {safeExternalUrl(item.url) && (
              <ExternalLink className="text-button" href={item.url}>
                阅读原文 <Icon name="external" size={15} />
              </ExternalLink>
            )}
            {item.attribution?.name === "AIHOT" && safeExternalUrl(item.attribution.url) && (
              <ExternalLink className="text-button" href={item.attribution.url}>
                发现渠道：AIHOT <Icon name="external" size={15} />
              </ExternalLink>
            )}
          </div>
        </details>
      ))}
    </div>
  );
}
