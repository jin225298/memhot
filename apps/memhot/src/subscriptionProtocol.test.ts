import { describe, expect, it } from "vitest";
import { decodeItems, decodeOpml, decodePreview } from "./subscriptionProtocol";
import { decodeSubscriptions } from "./subscriptionData";
describe("subscription response boundaries", () => {
  it("keeps unknown dates unknown and rejects unsafe article links", () => {
    const item = { id: "a", title: "Public article", url: "https://example.com/a", excerpt: "Summary", publishedAt: null };
    expect(decodeItems({ items: [item] })[0].publishedAt).toBeNull();
    for (const url of ["javascript:alert(1)", "https://secret@example.com/a"]) expect(() => decodeItems({ items: [{ ...item, url }] })).toThrow();
    expect(() => decodeItems({ items: [{ ...item, publishedAt: "today" }] })).toThrow();
  });
  it("rejects malformed lists before rendering", () => {
    expect(() => decodeSubscriptions({ subscriptions: [{}] })).toThrow();
    expect(() => decodePreview({ candidates: null, warnings: [] })).toThrow();
    expect(() => decodeOpml({ feeds: [], rejected: [], duplicateCount: -1 })).toThrow();
  });
  it("preserves unavailable and valid empty previews", () => {
    const empty = { title: "New feed", url: "https://example.com/feed", coverage: "empty", items: [] };
    expect(decodePreview({ candidates: [empty], warnings: [] }).candidates[0].items).toEqual([]);
    const error = { code: "timeout", message: "来源响应超时" };
    expect(decodePreview({ candidates: [{ ...empty, coverage: "unavailable", error }], warnings: [] }).candidates[0].error).toEqual(error);
  });
});
