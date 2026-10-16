import type { Home, Session } from "./types";
export const defaultServer =
  import.meta.env.DEV && import.meta.env.VITE_MEMHOT_SERVER
    ? normalizeServer(import.meta.env.VITE_MEMHOT_SERVER)
    : "https://memhot.angelkang.com";
export function normalizeServer(input: string) {
  const url = new URL(input.trim());
  if (
    url.protocol !== "https:" &&
    !(
      url.protocol === "http:" &&
      ["localhost", "127.0.0.1", "10.0.2.2"].includes(url.hostname)
    )
  )
    throw new Error("请使用 HTTPS 服务器地址");
  if (url.username || url.password || url.search || url.hash)
    throw new Error("服务器地址不能包含账号、参数或片段");
  return url.origin;
}
export class ApiError extends Error {
  constructor(
    message: string,
    public status: number,
  ) {
    super(message);
  }
}
export class Api {
  constructor(public readonly session: Session) {}
  async request<T>(
    path: string,
    method = "GET",
    body?: unknown,
    signal?: AbortSignal,
    idempotencyKey?: string,
  ): Promise<T> {
    const controller = new AbortController();
    const timer = setTimeout(
      () => controller.abort(),
      path === "/api/v1/discuss" ? 780000
        : path.includes("/models/test") ? 180000 : 30000,
    );
    const cancel = () => controller.abort();
    signal?.addEventListener("abort", cancel, { once: true });
    if (signal?.aborted) controller.abort();
    try {
      const response = await fetch(`${this.session.baseUrl}${path}`, {
        method,
        signal: controller.signal,
        headers: {
          ...(this.session.token
            ? { Authorization: `Bearer ${this.session.token}` }
            : {}),
          ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
          ...(idempotencyKey ? { "Idempotency-Key": idempotencyKey } : {}),
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      if (response.status === 204) return undefined as T;
      const data = await response.json().catch(() => null);
      if (!response.ok)
        throw new ApiError(
          data?.error?.message ||
            (response.status === 401
              ? "登录已过期，请重新登录"
              : "服务器暂时不可用，请稍后重试"),
          response.status,
        );
      if (data === null) throw new Error("服务器返回了无法读取的内容，请重试");
      return data as T;
    } catch (error) {
      if (error instanceof Error && error.name === "AbortError")
        throw new Error(
          signal?.aborted ? "请求已取消" : "等待服务器超时，请稍后重试",
        );
      if (error instanceof TypeError)
        throw new Error("连接失败，请检查网络和服务器地址");
      throw error;
    } finally {
      clearTimeout(timer);
      signal?.removeEventListener("abort", cancel);
    }
  }
  async home(signal?: AbortSignal): Promise<Home> {
    const home = await this.request<Home>(
      "/api/v1/home?thoughtScope=today",
      "GET",
      undefined,
      signal,
    );
    if (
      !home.profile ||
      ![home.thoughts, home.reports, home.sources, home.jobs].every(
        Array.isArray,
      )
    )
      throw new Error("数据格式不匹配，请检查服务器版本");
    return home;
  }
}
export function errorText(error: unknown) {
  return error instanceof Error ? error.message : "操作未完成，请重试";
}
export function shortDate(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? ""
    : date.toLocaleString("zh-CN", {
        month: "numeric",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
}
export function safeExternalUrl(value: string) {
  try {
    const u = new URL(value);
    return ["https:", "http:"].includes(u.protocol) &&
      !u.username &&
      !u.password
      ? u.href
      : undefined;
  } catch {
    return undefined;
  }
}
