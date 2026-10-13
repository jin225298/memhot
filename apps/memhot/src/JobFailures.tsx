import { useState } from "react";
import { Api, errorText } from "./api";
import { Notice } from "./components";
import type { Job } from "./types";

export function JobFailures({
  api,
  jobs,
  onChanged,
}: {
  api: Api;
  jobs: Job[];
  onChanged: () => void;
}) {
  const [pending, setPending] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState("");
  const [retried, setRetried] = useState<string[]>([]);
  const failed = jobs
    .filter((j) => j.status === "failed" || j.status === "blocked")
    .slice(0, 5);
  if (!failed.length) return null;
  async function retry(job: Job, accepted = false) {
    if (job.requiresChargeConfirmation && !accepted) {
      setConfirm(job.id);
      return;
    }
    setPending(job.id);
    setError("");
    try {
      await api.request(
        `/api/v1/jobs/${encodeURIComponent(job.id)}/retry`,
        "POST",
        { acceptDuplicateCharge: accepted },
      );
      setRetried((ids) => [...ids, job.id]);
      setConfirm("");
      onChanged();
    } catch (e) {
      setError(errorText(e));
      onChanged();
    } finally {
      setPending("");
    }
  }
  return (
    <details className="job-failures">
      <summary>有内容暂未处理完成 · {failed.length} 项</summary>
      {error && <Notice error={error} />}
      {failed.map((job) => (
        <div className="job-failure" key={job.id}>
          <p>{job.message || "处理未完成，请检查账户额度后重试。"}</p>
          {confirm === job.id ? (
            <div className="delete-confirm">
              <p>
                上次请求可能已被模型服务处理，但未收到结果。重新请求可能重复计费，确认再次提交？
              </p>
              <button
                className="secondary"
                disabled={Boolean(pending)}
                onClick={() => void retry(job, true)}
              >
                确认重新请求
              </button>
              <button
                className="text-button"
                disabled={Boolean(pending)}
                onClick={() => setConfirm("")}
              >
                取消
              </button>
            </div>
          ) : (
            <button
              className="text-button"
              disabled={
                Boolean(pending) ||
                job.retryable === false ||
                retried.includes(job.id)
              }
              onClick={() => void retry(job)}
            >
              {pending === job.id
                ? "正在重排…"
                : retried.includes(job.id) || job.retryable === false
                  ? "已重排或无法恢复"
                  : "重新处理"}
            </button>
          )}
        </div>
      ))}
    </details>
  );
}
