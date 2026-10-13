import { useEffect, useState } from "react";
import { Api, errorText, shortDate } from "./api";
import { Empty, Loading, Notice } from "./components";
import { Icon } from "./Icon";
import { SelectField } from "./ui/select";
import type { Report } from "./types";
import { localDay } from "./thoughtArchive";
import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "./ui/accordion";

interface Archive {
  reports: Report[];
  categories?: string[];
  nextCursor?: string | null;
  overview: {
    title: string;
    summary: string;
    reportCount: number;
    storyCount: number;
  };
}
export function ReadingArchive({
  api,
  kind,
  refreshKey,
  onReport,
  emptyAction,
  timezone,
}: {
  api: Api;
  kind: "research" | "digest";
  refreshKey: string;
  onReport: (id: string) => void;
  emptyAction: () => void;
  timezone: string;
}) {
  const [period, setPeriod] = useState("all");
  const [saved, setSaved] = useState(false);
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const [category, setCategory] = useState("");
  const [data, setData] = useState<Archive | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [cursor, setCursor] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => setSearch(query.trim()), 350);
    return () => clearTimeout(timer);
  }, [query]);
  const filter = `${kind}:${period}:${saved}:${search}:${category}:${refreshKey}:${attempt}`;
  const [loadedFilter, setLoadedFilter] = useState("");
  const current = loadedFilter === filter ? data : null;
  useEffect(() => {
    const controller = new AbortController();
    const next = loadedFilter === filter ? cursor : "";
    const params = new URLSearchParams({
      kind,
      period,
      saved: String(saved),
      q: search,
      category,
      limit: "20",
    });
    if (next) params.set("cursor", next);
    setBusy(true);
    setError("");
    api
      .request<Archive>(
        `/api/v1/reading?${params}`,
        "GET",
        undefined,
        controller.signal,
      )
      .then((value) => {
        if (controller.signal.aborted) return;
        setData((old) =>
          next && old
            ? {
                ...value,
                reports: [...old.reports, ...value.reports].filter(
                  (r, i, all) => all.findIndex((x) => x.id === r.id) === i,
                ),
              }
            : value,
        );
        setLoadedFilter(filter);
      })
      .catch((e) => {
        if (!controller.signal.aborted) setError(errorText(e));
      })
      .finally(() => {
        if (!controller.signal.aborted) setBusy(false);
      });
    return () => controller.abort();
    // loadedFilter is a response marker, not an independent request trigger.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, filter, cursor]);
  const filtering =
    saved || Boolean(search) || Boolean(category) || period !== "all";
  const reports = current?.reports || [];
  const complete = reports.filter((r) => r.status !== "incomplete");
  const incomplete = reports.filter((r) => r.status === "incomplete");
  const dates = [
    ...new Set(complete.map((r) => localDay(r.createdAt, timezone))),
  ]
    .sort()
    .reverse();
  return (
    <section className="reading-archive" aria-label="阅读资料库">
      <div className="archive-controls">
        <label className="archive-search">
          <Icon name="search" size={17} />
          <input
            type="search"
            value={query}
            maxLength={200}
            onChange={(e) => {
              setCursor("");
              setQuery(e.target.value);
            }}
            placeholder="搜索标题与内容"
            aria-label="搜索阅读内容"
          />
        </label>
        <SelectField
          label="阅读时间范围"
          value={period}
          onValueChange={(v) => {
            setCursor("");
            setPeriod(v);
          }}
          options={[
            { value: "all", label: "全部时间" },
            { value: "week", label: "近一周" },
            { value: "month", label: "近一月" },
          ]}
        />
        <button
          className={`archive-saved ${saved ? "selected" : ""}`}
          aria-pressed={saved}
          onClick={() => {
            setCursor("");
            setSaved((v) => !v);
          }}
        >
          <Icon name="bookmark" size={17} /> 收藏
        </button>
      </div>
      {(data?.categories?.length ?? 0) > 1 && (
        <div className="archive-categories" aria-label="内容分类">
          {["", ...(data?.categories || [])].map((value) => (
            <button
              key={value}
              className="category-chip"
              aria-pressed={category === value}
              onClick={() => {
                setCursor("");
                setCategory(value);
              }}
            >
              {value || "全部分类"}
            </button>
          ))}
        </div>
      )}
      {error && (
        <Notice error={error} onRetry={() => setAttempt((n) => n + 1)} />
      )}
      {!current && busy ? (
        <Loading />
      ) : current && reports.length === 0 ? (
        <Empty
          icon={saved ? "bookmark" : "book"}
          title={
            filtering
              ? "没有找到符合条件的内容"
              : kind === "digest"
                ? "还没有日报"
                : "还没有研究材料"
          }
          action={
            <button
              className="secondary"
              onClick={() => {
                if (filtering) {
                  setCursor("");
                  setSaved(false);
                  setPeriod("all");
                  setQuery("");
                  setSearch("");
                  setCategory("");
                } else emptyAction();
              }}
            >
              {filtering
                ? "清除筛选"
                : kind === "digest"
                  ? "选择关注领域"
                  : "去录音"}
            </button>
          }
        >
          {filtering
            ? "换个关键词，或看看其他时间的资料。"
            : kind === "digest"
              ? "选好关注领域，每天来这里看新进展。"
              : "留下问题，相关研究会出现在这里。"}
        </Empty>
      ) : (
        <>
          {period !== "all" && current && (
            <div className="archive-overview">
              <span className="eyebrow">{current.overview.title}</span>
              <p>{current.overview.summary}</p>
              <small>根据已收到的内容汇总</small>
            </div>
          )}
          <Accordion
            key={filter}
            className="reading-date-groups"
            type="multiple"
            defaultValue={filtering ? dates : dates.slice(0, 1)}
            aria-busy={busy}
          >
            {dates.map((date) => (
              <AccordionItem
                key={date}
                value={date}
                className="reading-date-group"
              >
                <AccordionTrigger className="collection-heading">
                  <span>
                    <strong>{date.replaceAll("-", ".")}</strong>
                    <small>
                      {
                        complete.filter(
                          (r) => localDay(r.createdAt, timezone) === date,
                        ).length
                      }{" "}
                      份{kind === "digest" ? "日报" : "研究"}
                    </small>
                  </span>
                </AccordionTrigger>
                <AccordionContent>
                  <div className="report-list">
                    {complete
                      .filter((r) => localDay(r.createdAt, timezone) === date)
                      .map((report) => (
                        <ReadingItem
                          key={report.id}
                          report={report}
                          onOpen={() => onReport(report.id)}
                        />
                      ))}
                  </div>
                </AccordionContent>
              </AccordionItem>
            ))}
          </Accordion>
          {incomplete.length > 0 && (
            <details className="unfinished-reports">
              <summary>
                {incomplete.length} 份未完成的内容 <span>查看记录</span>
              </summary>
              <p>
                这些记录只保留了部分资料，尚未形成完整报告。可通过上方“更新”重新生成。
              </p>
              <div className="report-list">
                {incomplete.map((report) => (
                  <ReadingItem
                    key={report.id}
                    report={report}
                    onOpen={() => onReport(report.id)}
                  />
                ))}
              </div>
            </details>
          )}
          {current?.nextCursor && (
            <button
              className="secondary archive-more"
              disabled={busy}
              onClick={() => setCursor(current.nextCursor || "")}
            >
              {busy ? "正在加载…" : "加载更多"}
            </button>
          )}
        </>
      )}
    </section>
  );
}

function ReadingItem({
  report,
  onOpen,
}: {
  report: Report;
  onOpen: () => void;
}) {
  const metrics = report.editorial?.metrics;
  return (
    <button className="report-row" onClick={onOpen}>
      <div className="report-label">
        <span>
          {report.status === "incomplete"
            ? "未完成"
            : report.kind === "digest"
              ? "兴趣日报"
              : "深度研究"}
          {report.saved && " · 已收藏"}
        </span>
        <time>{shortDate(report.createdAt)}</time>
      </div>
      <h2>{report.title}</h2>
      <p className="report-excerpt">{report.summary}</p>
      <div className="report-footer">
        <span>
          {metrics
            ? `${metrics.storyCount} 件进展 · 约 ${metrics.readingMinutes} 分钟`
            : `${report.sourceCount ?? report.sources?.length ?? 0} 份资料`}
          {report.status === "partial" && " · 部分问题待验证"}
        </span>
        <span className="read-action">
          阅读 <Icon name="arrow" size={17} />
        </span>
      </div>
    </button>
  );
}
