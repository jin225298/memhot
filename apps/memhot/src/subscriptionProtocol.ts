import type { CatalogSource, FeedCandidate, FeedInput, OpmlPreview } from "./subscriptionData";
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("来源数据暂时无法读取，请重试");
  return value as Record<string, unknown>;
}
function text(value: unknown): string {
  if (typeof value !== "string") throw new Error("来源数据暂时无法读取，请重试");
  return value;
}
function array(value: unknown): unknown[] {
  if (!Array.isArray(value)) throw new Error("来源列表暂时无法读取，请重试");
  return value;
}
function url(value: unknown): string {
  const parsed = new URL(text(value));
  if (!["https:", "http:"].includes(parsed.protocol) || parsed.username || parsed.password) throw new Error("来源返回了无效的链接");
  return parsed.toString();
}
function date(value: unknown): string | null {
  if (value === null) return null;
  const raw = text(value);
  if (!Number.isFinite(Date.parse(raw))) throw new Error("来源返回了无效的日期");
  return raw;
}
function input(value: unknown): FeedInput {
  const s = object(value);
  return { url: url(s.url), title: text(s.title), category: text(s.category) };
}
export function decodeCatalog(value: unknown): CatalogSource[] {
  return array(object(value).sources).map(raw => {
    const s = object(raw);
    return { ...input(s), id: text(s.id), description: text(s.description) };
  });
}
export function decodePreview(value: unknown): { candidates: FeedCandidate[]; warnings: string[] } {
  const data = object(value);
  const candidates = array(data.candidates).map(raw => {
    const s = object(raw);
    const coverage = text(s.coverage) as FeedCandidate["coverage"];
    if (!["summary", "title_only", "empty", "unavailable"].includes(coverage)) throw new Error("暂时无法确认这个来源提供的内容");
    const result: FeedCandidate = { url: url(s.url), title: text(s.title), coverage,
      items: array(s.items).map(rawItem => { const i = object(rawItem); return { title: text(i.title), url: url(i.url), summary: text(i.summary), publishedAt: date(i.publishedAt) }; }) };
    if (s.error) { const e = object(s.error); result.error = { code: text(e.code), message: text(e.message) }; }
    return result;
  });
  return { candidates, warnings: array(data.warnings).map(text) };
}
export function decodeOpml(value: unknown): OpmlPreview {
  const data = object(value);
  if (!Number.isInteger(data.duplicateCount) || Number(data.duplicateCount) < 0) throw new Error("导入结果暂时无法读取，请重试");
  return { feeds: array(data.feeds).map(input), duplicateCount: Number(data.duplicateCount),
    rejected: array(data.rejected).map(raw => { const s = object(raw); return { title: text(s.title), reason: text(s.reason) }; }) };
}
export interface SubscriptionItem { id: string; title: string; url: string; excerpt: string; publishedAt: string | null }
export function decodeItems(value: unknown): SubscriptionItem[] {
  return array(object(value).items).map(raw => {
    const s = object(raw);
    return { id: text(s.id), title: text(s.title), url: url(s.url), excerpt: text(s.excerpt), publishedAt: date(s.publishedAt) };
  });
}
