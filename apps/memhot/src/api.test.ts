import { afterEach, describe, expect, it, vi } from "vitest";
import { Api, ApiError, normalizeServer, safeExternalUrl } from "./api";
import { emptyHome } from "./types";
afterEach(() => vi.unstubAllGlobals());
describe("server boundary", () => {
  it("requires a secure origin and rejects embedded credentials", () => {
    expect(normalizeServer("https://memhot.angelkang.com/")).toBe(
      "https://memhot.angelkang.com",
    );
    expect(normalizeServer("http://127.0.0.1:8790")).toBe(
      "http://127.0.0.1:8790",
    );
    for (const value of [
      "http://remote.example.com",
      "https://user:secret@example.com",
      "https://example.com?token=secret",
      "javascript:alert(1)",
    ])
      expect(() => normalizeServer(value)).toThrow();
  });
  it("never renders executable or credential-bearing source URLs", () => {
    expect(safeExternalUrl("javascript:alert(1)")).toBeUndefined();
    expect(safeExternalUrl("data:text/html,hello")).toBeUndefined();
    expect(safeExternalUrl("https://secret:token@example.com")).toBeUndefined();
    expect(safeExternalUrl("https://example.com/report")).toBe(
      "https://example.com/report",
    );
  });
});
describe("API failure semantics", () => {
  const api = new Api({ baseUrl: "https://example.com", token: "test-token" });
  it("propagates unauthorized status for reconnect flow", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(JSON.stringify({ error: { message: "请重新配对" } }), {
          status: 401,
        }),
      ),
    );
    await expect(api.home()).rejects.toEqual(new ApiError("请重新配对", 401));
  });
  it("rejects a mismatched home protocol rather than crashing during rendering", async () => {
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValue(
          new Response(JSON.stringify({ profile: {}, reports: {} })),
        ),
    );
    await expect(api.home()).rejects.toThrow("数据格式不匹配");
  });
  it("preserves authorization and JSON boundaries", async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify(emptyHome)));
    vi.stubGlobal("fetch", fetcher);
    expect(await api.home()).toEqual(emptyHome);
    expect(fetcher).toHaveBeenCalledWith(
      "https://example.com/api/v1/home?thoughtScope=today",
      expect.objectContaining({
        headers: { Authorization: "Bearer test-token" },
        method: "GET",
      }),
    );
  });
  it("handles empty deletion responses without attempting JSON parsing", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response(null, { status: 204 })),
    );
    await expect(
      api.request("/api/v1/sources/test", "DELETE"),
    ).resolves.toBeUndefined();
  });
});
