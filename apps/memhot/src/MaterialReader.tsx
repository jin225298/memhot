import { useEffect, useState } from "react";
import { Api, errorText } from "./api";
import { Loading, Notice } from "./components";
import { ExternalLink } from "./ExternalLink";
import { Icon } from "./Icon";

interface Material {
  title: string;
  url: string;
  publisher?: string;
  attribution?: { name: string; url: string } | null;
  text: string;
  availability: "full" | "excerpt";
  retrievalKind: string;
  truncated: boolean;
  translation?: {
    status: string;
    text?: string;
    jobId?: string;
    message?: string;
  };
}
export function MaterialReader({
  api,
  reportId,
  evidenceId,
  onBack,
}: {
  api: Api;
  reportId: string;
  evidenceId: string;
  onBack: () => void;
}) {
  const [data, setData] = useState<Material | null>(null);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [request, setRequest] = useState(false);
  const [translated, setTranslated] = useState(false);
  const path = `/api/v1/reports/${encodeURIComponent(reportId)}/materials/${encodeURIComponent(evidenceId)}`;
  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    let polls = 0;
    const load = async () => {
      try {
        const value = await api.request<Material>(
          path,
          "GET",
          undefined,
          controller.signal,
        );
        if (controller.signal.aborted) return;
        setData(value);
        setError("");
        if (
          ["queued", "running"].includes(value.translation?.status || "") &&
          polls++ < 60
        )
          timer = setTimeout(() => void load(), 5000);
      } catch (e) {
        if (!controller.signal.aborted) setError(errorText(e));
      }
    };
    void load();
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, [api, path, attempt]);
  async function translate() {
    setRequest(true);
    setError("");
    try {
      await api.request(`${path}/translation`, "POST", {});
      setTranslated(true);
      setAttempt((n) => n + 1);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setRequest(false);
    }
  }
  const pending = ["queued", "running"].includes(
    data?.translation?.status || "",
  );
  const translationFailed = ["failed", "blocked"].includes(
    data?.translation?.status || "",
  );
  return (
    <article className="material-reader">
      <button className="text-button back-button" onClick={onBack}>
        <Icon name="back" size={18} />
        返回报告
      </button>
      {error && (
        <Notice error={error} onRetry={() => setAttempt((n) => n + 1)} />
      )}
      {!data ? (
        !error && <Loading />
      ) : (
        <>
          <div className="report-label">
            <span>{data.publisher || "原始资料"}</span>
            <span>
              {data.availability === "full" ? "网页正文" : "资料摘录"}
            </span>
          </div>
          <h1>{data.title}</h1>
          {data.attribution?.name === "AIHOT" && (
            <p className="meta">
              发现渠道：<ExternalLink href={data.attribution.url}>AIHOT</ExternalLink>
            </p>
          )}
          {(data.availability === "excerpt" || data.truncated) && (
            <p className="partial-note">
              {data.availability === "excerpt"
                ? "这里只保存了摘要或摘录，完整内容请查看原文。"
                : "本文较长，这里展示已保存的部分正文。"}
            </p>
          )}
          <div className="material-toolbar">
            <ExternalLink className="text-button" href={data.url}>
              打开原文 <Icon name="external" size={16} />
            </ExternalLink>
            {data.translation?.text ? (
              <button
                className="secondary"
                aria-pressed={translated}
                onClick={() => setTranslated((v) => !v)}
              >
                {translated ? "查看原文" : "查看中文"}
              </button>
            ) : (
              <button
                className="secondary"
                disabled={request || pending || translationFailed}
                onClick={() => void translate()}
              >
                {request ? "提交中…" : pending ? "正在翻译…" : "翻译成中文"}
              </button>
            )}
          </div>
          {!data.translation?.text && !pending && !translationFailed && (
            <p className="material-translation-note">
              按需翻译，会使用账户额度。
            </p>
          )}
          {pending && (
            <p role="status" className="material-translation-note">
              翻译完成后会自动显示，也可以稍后回来查看。
            </p>
          )}
          {translationFailed && (
            <Notice
              error={
                data.translation?.message ||
                "翻译未完成，可在任务记录中查看并重试。"
              }
            />
          )}
          {translated && data.translation?.text && (
            <p className="material-translation-note">
              以下为机器翻译，请以原文为准。
            </p>
          )}
          <div className="material-text">
            {(translated && data.translation?.text
              ? data.translation.text
              : data.text
            )
              .split(/\n\s*\n/)
              .map((p, i) => (
                <p key={i}>{p}</p>
              ))}
          </div>
        </>
      )}
    </article>
  );
}
