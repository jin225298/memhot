import type { Thought } from "./types";

export function localDay(date: string, timezone: string): string {
  if (!Number.isFinite(Date.parse(date))) return "日期未知";
  return new Intl.DateTimeFormat("sv-SE", {
    timeZone: timezone || "Asia/Shanghai",
  }).format(new Date(date));
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
        : localDay(thought.createdAt, timezone);
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
