import type { Thought } from "./types";

export function localDay(date: string, timezone: string): string {
  if (!Number.isFinite(Date.parse(date))) return "日期未知";
  return new Intl.DateTimeFormat("sv-SE", {
    timeZone: timezone || "Asia/Shanghai",
  }).format(new Date(date));
}

export function thoughtDay(thought: Thought, timezone: string): string {
  return thought.thinkingDate && /^\d{4}-\d{2}-\d{2}$/.test(thought.thinkingDate)
    ? thought.thinkingDate
    : localDay(thought.createdAt, timezone);
}

export function groupThoughts(
  thoughts: Thought[],
  timezone: string,
  by: "date" | "category",
) {
  const grouped = new Map<string, Thought[]>();
  for (const thought of thoughts) {
    const key =
      by === "category"
        ? thought.category?.trim() || "未分类"
        : thoughtDay(thought, timezone);
    grouped.set(key, [...(grouped.get(key) || []), thought]);
  }
  return [...grouped]
    .sort(([a], [b]) =>
      by === "date" ? b.localeCompare(a) : a.localeCompare(b, "zh-CN"),
    )
    .map(([key, items]) => ({
      key,
      label: by === "date" ? key.replaceAll("-", ".") : key,
      items,
    }));
}
