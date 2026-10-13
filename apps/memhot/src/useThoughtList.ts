import { useEffect, useRef, useState } from "react";
import { Api, errorText } from "./api";
import type { Thought } from "./types";
interface Page {
  items: Thought[];
  nextCursor: string | null;
}
export function useThoughtList(api: Api, recent: Thought[]) {
  const [items, setItems] = useState(recent);
  const [next, setNext] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [version, setVersion] = useState(0);
  const controller = useRef<AbortController | null>(null);
  const signature = JSON.stringify(
    recent.map((t) => [t.id, t.revision, t.status, t.analysisStatus]),
  );
  useEffect(() => {
    const active = new AbortController();
    controller.current?.abort();
    controller.current = active;
    setLoading(true);
    setError("");
    api
      .request<Page>(
        "/api/v1/thoughts?limit=50",
        "GET",
        undefined,
        active.signal,
      )
      .then((page) => {
        if (!active.signal.aborted) {
          setItems(page.items);
          setNext(page.nextCursor);
        }
      })
      .catch((e) => {
        if (!active.signal.aborted) setError(errorText(e));
      })
      .finally(() => {
        if (!active.signal.aborted) setLoading(false);
      });
    return () => active.abort();
  }, [api, signature, version]);
  async function more() {
    if (!next || loading) return;
    const active = new AbortController();
    controller.current?.abort();
    controller.current = active;
    setLoading(true);
    setError("");
    try {
      const page = await api.request<Page>(
        `/api/v1/thoughts?limit=50&before=${encodeURIComponent(next)}`,
        "GET",
        undefined,
        active.signal,
      );
      if (!active.signal.aborted) {
        setItems((old) => [
          ...old,
          ...page.items.filter((t) => !old.some((o) => o.id === t.id)),
        ]);
        setNext(page.nextCursor);
      }
    } catch (e) {
      if (!active.signal.aborted) setError(errorText(e));
    } finally {
      if (!active.signal.aborted) setLoading(false);
    }
  }
  useEffect(() => () => controller.current?.abort(), []);
  return {
    items,
    next,
    loading,
    error,
    more,
    reload: () => setVersion((v) => v + 1),
  };
}
