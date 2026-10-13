import { useEffect, useState } from "react";
import { Api, errorText, shortDate } from "./api";
import { EvidenceList, Loading, Notice } from "./components";
import { Editorial } from "./Editorial";
import { MaterialReader } from "./MaterialReader";
import { EventHistory } from "./EventHistory";
import { Icon } from "./Icon";
import type { Report } from "./types";
import { useBackAction } from "./useBackAction";
import { backLayer } from "./backNavigation";
import { Markdown } from "./Markdown";
import { ExternalLink } from "./ExternalLink";
export function Reader({
  api,
  reportId,
  onBack,
  onDiscuss,
  backLabel,
  onReport,
}: {
  api: Api;
  reportId: string;
  onBack: () => void;
  onDiscuss: (report: Report, question?: string) => void;
  backLabel: string;
  onReport: (id: string) => void;
}) {
  const [report, setReport] = useState<Report | null>(null);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [view, setView] = useState<{
    kind: "material" | "event";
    id: string;
  } | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState("");
  useBackAction(Boolean(view), () => setView(null), backLayer.detail);
  async function toggleSaved() {
    if (!report || saving) return;
    const expected = report.id;
    setSaving(true);
    setSaveError("");
    try {
      await api.request(
        `/api/v1/reports/${encodeURIComponent(expected)}/saved`,
        "PUT",
        { saved: !report.saved },
      );
      setReport((value) =>
        value?.id === expected ? { ...value, saved: !value.saved } : value,
      );
    } catch (e) {
      setSaveError(errorText(e));
    } finally {
      setSaving(false);
    }
  }
  // A report body can already include its gaps; show only additional prompts here.
  const followUps =
    report?.gaps?.filter((gap) => !report.body.includes(gap)) || [];
  useEffect(() => {
    const controller = new AbortController();
    setReport(null);
    setView(null);
    setSaveError("");
    setError("");
    api
      .request<Report>(
        `/api/v1/reports/${encodeURIComponent(reportId)}`,
        "GET",
        undefined,
        controller.signal,
      )
      .then((r) => {
        if (!controller.signal.aborted) setReport(r);
      })
      .catch((e) => {
        if (!controller.signal.aborted) setError(errorText(e));
      });
    return () => controller.abort();
  }, [api, reportId, attempt]);
  if (view?.kind === "material")
    return (
      <MaterialReader
        key={view.id}
        api={api}
        reportId={reportId}
        evidenceId={view.id}
        onBack={() => setView(null)}
      />
    );
  if (view?.kind === "event")
    return (
      <EventHistory
        key={view.id}
        api={api}
        reportId={reportId}
        storyId={view.id}
        onBack={() => setView(null)}
        onReport={onReport}
      />
    );
  return (
    <article className="reader">
      <button className="text-button back-button" onClick={onBack}>
        <Icon name="back" size={18} />
        {backLabel}
      </button>
      {error ? (
        <Notice error={error} onRetry={() => setAttempt((x) => x + 1)} />
      ) : !report ? (
        <Loading />
      ) : (
        <>
          <header className="report-header">
            <div className="report-label reader-meta">
              <span>
                {report.kind === "research" ? "深度研究" : "兴趣日报"}
              </span>
              <span>{shortDate(report.createdAt)}</span>
            </div>
            <h1>{report.title}</h1>
            <div className="reader-tools">
              {report.editorial?.metrics ? (
                <span>
                  {report.editorial.metrics.storyCount} 件进展 ·{" "}
                  {report.editorial.metrics.publisherCount} 个来源机构 · 约{" "}
                  {report.editorial.metrics.readingMinutes} 分钟
                </span>
              ) : (
                <span>
                  {report.sources?.length || 0} 份资料 · 约{" "}
                  {Math.max(
                    1,
                    Math.ceil(
                      (report.body.length + report.summary.length) / 500,
                    ),
                  )}{" "}
                  分钟
                </span>
              )}
              <button
                className="text-button"
                aria-pressed={Boolean(report.saved)}
                disabled={saving}
                onClick={() => void toggleSaved()}
              >
                <Icon name="bookmark" size={17} />
                {saving ? "保存中…" : report.saved ? "已收藏" : "收藏"}
              </button>
            </div>
            {report.contentOrigin === "external_editorial" &&
              report.attribution && (
                <p className="report-attribution">
                  本期内容来自{" "}
                  <ExternalLink href={report.attribution.url}>
                    {report.attribution.name}
                  </ExternalLink>
                </p>
              )}
          </header>
          {saveError && <Notice error={saveError} />}
          {report.status === "incomplete" && (
            <p className="partial-note">
              这份内容尚未完成分析，目前只保留了部分资料。
            </p>
          )}
          {report.summary && (
            <div className="reader-summary">
              <Markdown text={report.summary} />
            </div>
          )}
          {report.status === "partial" && (
            <p className="partial-note">
              这份材料仍有证据缺口，请结合原始来源判断。
            </p>
          )}
          {report.question && (
            <details className="original-record report-question">
              <summary>
                <Icon name="thought" size={16} />
                这份材料回应的问题
                <Icon name="chevron" size={16} />
              </summary>
              <p>{report.question}</p>
            </details>
          )}
          <div className="report-paper">
            <Editorial
              report={report}
              onDiscuss={(question) => onDiscuss(report, question)}
              onMaterial={(id) => setView({ kind: "material", id })}
              onEvent={(id) => setView({ kind: "event", id })}
            />
            {followUps.length > 0 && (
              <section className="unanswered">
                <h2>仍然值得追问</h2>
                <ul>
                  {followUps.map((gap, i) => (
                    <li key={i}>{gap}</li>
                  ))}
                </ul>
              </section>
            )}
            <section className="reader-sources">
              <div className="section-heading">
                <h2>材料与出处</h2>
                <span className="muted">
                  {report.sources?.length || 0} 份来源
                </span>
              </div>
              <EvidenceList
                evidence={report.sources || []}
                onRead={(id) => setView({ kind: "material", id })}
              />
              {!report.sources?.length && (
                <p className="muted">
                  本次没有获得可核验的外部来源，不能把生成内容当作已验证事实。
                </p>
              )}
            </section>
          </div>
          <div className="continue-discussion">
            <div>
              <h2>有新的想法了？</h2>
              <p>带着这些材料，继续展开你的问题。</p>
            </div>
            <button className="primary" onClick={() => onDiscuss(report)}>
              <Icon name="chat" size={18} />
              接着聊
            </button>
          </div>
        </>
      )}
    </article>
  );
}
