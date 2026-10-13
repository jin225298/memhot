import { Api } from "./api";

export interface Subscription {
  id: string;
  title: string;
  host: string;
  category: string;
  enabled: boolean;
  status: "pending" | "collecting" | "healthy" | "error" | "disabled";
  lastAttemptAt: string | null;
  lastSuccessAt: string | null;
  nextAttemptAt: string | null;
  itemCount: number;
  lastNewCount: number;
  error: string | null;
  shared: boolean;
}
export interface FeedInput { url: string; title: string; category: string }
export interface CatalogSource extends FeedInput { id: string; description: string }
export interface FeedCandidate {
  url: string;
  title: string;
  description?: string;
  coverage: "summary" | "title_only" | "empty" | "unavailable";
  items: { title: string; url: string; publishedAt: string | null; summary: string }[];
  error?: { code: string; message: string };
}
export interface OpmlPreview {
  feeds: FeedInput[];
  rejected: { title: string; reason: string }[];
  duplicateCount: number;
}
const statuses = ["pending", "collecting", "healthy", "error", "disabled"];

/** The subscription protocol is validated before becoming view state. */
export function decodeSubscriptions(value: unknown): Subscription[] {
  const data = value as { subscriptions?: unknown } | null;
  if (!data || !Array.isArray(data.subscriptions)) throw new Error("订阅列表暂时无法读取，请重试");
  return data.subscriptions.map((raw: unknown) => {
    const s = raw as Subscription | null;
    if (!s || ![s.id, s.title, s.host, s.category].every(v => typeof v === "string") ||
        typeof s.enabled !== "boolean" || typeof s.shared !== "boolean" ||
        !statuses.includes(s.status) || ![s.itemCount, s.lastNewCount].every(v => Number.isInteger(v) && v >= 0) ||
        ![s.lastAttemptAt, s.lastSuccessAt, s.nextAttemptAt].every(v => v === null || (typeof v === "string" && Number.isFinite(Date.parse(v)))) ||
        !(s.error === null || typeof s.error === "string")) {
      throw new Error("订阅状态暂时无法读取，请重试");
    }
    return { ...s };
  });
}
export async function listSubscriptions(api: Api, signal?: AbortSignal) {
  return decodeSubscriptions(await api.request<unknown>("/api/v1/subscriptions", "GET", undefined, signal));
}

export const subscriptionStatus: Record<Subscription["status"], string> = {
  pending: "等待更新", collecting: "正在更新", healthy: "正常更新", error: "更新失败", disabled: "已暂停",
};
