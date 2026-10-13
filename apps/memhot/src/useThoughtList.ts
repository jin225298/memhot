import { useEffect, useRef, useState } from "react";
import { Api, errorText } from "./api";
import type { Thought } from "./types";

export interface ThoughtFilters {
  status: "active" | "archived";
  q: string;
  period: string;
  category: string;
  date: string;
}
interface Page {
  items: Thought[];
  nextCursor: string | null;
  total: number;
  categories: string[];
  dates: string[];
  counts: { active: number; archived: number };
}
const empty: Page = {
  items: [],
  nextCursor: null,
  total: 0,
  categories: [],
  dates: [],
  counts: { active: 0, archived: 0 },
};

export function useThoughtList(
  api: Api,
  recent: Thought[],
  filters: ThoughtFilters,
) {
  const [data, setData] = useState<Page>(empty);
  const [loaded, setLoaded] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [version, setVersion] = useState(0);
  const controller = useRef<AbortController | null>(null);
  const params = new URLSearchParams({
    status: filters.status,
    period: filters.period,
    limit: "30",
  });
  for (const field of ["q", "category", "date"] as const) {
    if (filters[field]) params.set(field, filters[field]);
  }
  const query = params.toString();
  const signature = JSON.stringify(
    recent.map((t) => [t.id, t.revision, t.status, t.analysisStatus]),
  );
  const generation = `${query}:${signature}:${version}`;
  const currentGeneration = useRef(generation);
  currentGeneration.current = generation;
  useEffect(() => {
    const active = new AbortController();
    controller.current?.abort();
    controller.current = active;
    setLoading(true);
    setError("");
    api
      .request<Page>(
        `/api/v1/thoughts?${query}`,
        "GET",
        undefined,
        active.signal,
      )
      .then((page) => {
        if (
          !active.signal.aborted &&
          currentGeneration.current === generation
        ) {
          setData(page);
          setLoaded(generation);
        }
      })
      .catch((e) => {
        if (!active.signal.aborted) setError(errorText(e));
      })
      .finally(() => {
        if (!active.signal.aborted) setLoading(false);
      });
    return () => active.abort();
  }, [api, query, generation]);
  async function more() {
    if (!data.nextCursor || loading || loaded !== generation) return;
    const active = new AbortController();
    controller.current?.abort();
    controller.current = active;
    setLoading(true);
    setError("");
    try {
      const page = await api.request<Page>(
        `/api/v1/thoughts?${query}&before=${encodeURIComponent(data.nextCursor)}`,
        "GET",
        undefined,
        active.signal,
      );
      if (!active.signal.aborted && currentGeneration.current === generation) {
        setData((old) => ({
          ...page,
          items: [
            ...old.items,
            ...page.items.filter((t) => !old.items.some((o) => o.id === t.id)),
          ],
        }));
      }
    } catch (e) {
      if (!active.signal.aborted) setError(errorText(e));
    } finally {
      if (!active.signal.aborted) setLoading(false);
    }
  }
  useEffect(() => () => controller.current?.abort(), []);
  return {
    ...data,
    items: loaded === generation ? data.items : [],
    next: loaded === generation ? data.nextCursor : null,
    ready: loaded === generation,
    loading,
    error,
    more,
    reload: () => setVersion((v) => v + 1),
  };
}
