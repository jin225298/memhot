import { useEffect, useState } from "react";
import { Api, errorText, shortDate } from "./api";
import { Loading, Notice } from "./components";
import { Icon } from "./Icon";
import type { EditorialStory } from "./types";

interface History {
  story: EditorialStory;
  related: {
    reportId: string;
    storyId: string;
    title: string;
    summary: string;
    date: string;
    relationship: "same_event" | "related";
  }[];
}
export function EventHistory({
  api,
  reportId,
  storyId,
  onBack,
  onReport,
}: {
  api: Api;
  reportId: string;
  storyId: string;
  onBack: () => void;
  onReport: (id: string) => void;
}) {
  const [data, setData] = useState<History | null>(null);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    api
      .request<History>(
        `/api/v1/reports/${encodeURIComponent(reportId)}/events/${encodeURIComponent(storyId)}`,
        "GET",
        undefined,
        controller.signal,
      )
      .then((v) => {
        if (!controller.signal.aborted) {
          setData(v);
          setError("");
        }
      })
      .catch((e) => {
        if (!controller.signal.aborted) setError(errorText(e));
      });
    return () => controller.abort();
  }, [api, reportId, storyId, attempt]);
  return (
    <section className="event-history">
      <button className="text-button back-button" onClick={onBack}>
        <Icon name="back" size={18} />
        返回报告
      </button>
      {error ? (
        <Notice error={error} onRetry={() => setAttempt((n) => n + 1)} />
      ) : !data ? (
        <Loading />
      ) : (
        <>
          <p className="eyebrow">相关进展</p>
          <h1>{data.story.title}</h1>
          <p className="reader-summary">{data.story.summary}</p>
          {data.related.length ? (
            <div className="event-history-list">
              {data.related.map((r) => (
                <button
                  key={`${r.reportId}:${r.storyId}`}
                  onClick={() => onReport(r.reportId)}
                >
                  <span className="event-history-dot" />
                  <small>
                    {shortDate(r.date)} ·{" "}
                    {r.relationship === "same_event" ? "同一事件" : "相关报道"}
                  </small>
                  <h2>{r.title}</h2>
                  <p>{r.summary}</p>
                  <span className="text-button">
                    阅读这期 <Icon name="arrow" size={15} />
                  </span>
                </button>
              ))}
            </div>
          ) : (
            <div className="archive-overview">
              <p>你的往期报告里，还没有找到相关报道。</p>
              <small>后续收到新的内容，会在这里串联起来。</small>
            </div>
          )}
        </>
      )}
    </section>
  );
}
