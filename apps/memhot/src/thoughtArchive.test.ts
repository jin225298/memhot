import { describe, expect, it } from "vitest";
import { groupThoughts, localDay } from "./thoughtArchive";
import type { Thought } from "./types";
const thought = (id: string, createdAt: string, category?: string) =>
  ({ id, createdAt, category }) as Thought;
describe("thought archive", () => {
  it("uses the profile timezone at midnight and keeps date groups newest first", () => {
    const groups = groupThoughts(
      [
        thought("before", "2026-10-12T15:59:00Z"),
        thought("after", "2026-10-12T16:01:00Z"),
      ],
      "Asia/Shanghai",
      "date",
    );
    expect(groups.map((g) => [g.key, g.items[0].id])).toEqual([
      ["2026-10-13", "after"],
      ["2026-10-12", "before"],
    ]);
    expect(localDay("2026-10-12T16:01:00Z", "America/Los_Angeles")).toBe(
      "2026-10-12",
    );
  });
  it("retains uncategorized and invalid-date legacy records", () => {
    const data = [
      thought("old", "bad"),
      thought("work", "2026-10-12", " 工作 "),
      thought("work2", "2026-10-13", "工作"),
    ];
    expect(
      groupThoughts(data, "Asia/Shanghai", "category").find(
        (g) => g.key === "工作",
      )?.items,
    ).toHaveLength(2);
    expect(
      groupThoughts(data, "Asia/Shanghai", "date").some(
        (g) => g.key === "日期未知",
      ),
    ).toBe(true);
    expect(
      groupThoughts(data, "Asia/Shanghai", "category").some(
        (g) => g.key === "未分类",
      ),
    ).toBe(true);
  });
});
