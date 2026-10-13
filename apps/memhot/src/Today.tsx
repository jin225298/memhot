import { Icon } from "./Icon";
import { Api } from "./api";
import { ReadingArchive } from "./ReadingArchive";
import { JobFailures } from "./JobFailures";
import type { Home } from "./types";
import type { Tab } from "./AppFrame";

export function Today({
  api,
  onChanged,
  onPreferences,
  home,
  loading,
  busy,
  navigate,
  onReport,
  onResearch,
  category: chosenCategory,
  onCategory,
}: {
  api: Api;
  onChanged: () => void;
  onPreferences: () => void;
  home: Home;
  loading: boolean;
  busy: boolean;
  navigate: (tab: Tab) => void;
  onReport: (id: string) => void;
  onResearch: (kind: "research" | "digest") => Promise<void>;
  category: "research" | "digest" | null;
  onCategory: (category: "research" | "digest") => void;
}) {
  const activeJobs = home.jobs.filter(
    (j) => j.status === "queued" || j.status === "running",
  );
  const research = home.reports.filter((r) => r.kind === "research");
  const digests = home.reports.filter((r) => r.kind === "digest");
  const category =
    chosenCategory ??
    (research.length || !digests.length ? "research" : "digest");
  const date = new Date().toLocaleDateString("zh-CN", {
    month: "long",
    day: "numeric",
    weekday: "long",
  });
  return (
    <>
      <div className="page-heading today-heading">
        <div>
          <h1>阅读</h1>
          <p className="muted">你的研究材料和领域日报。</p>
        </div>
        <time className="today-date">{date}</time>
      </div>
      {activeJobs.length > 0 && (
        <div className="job-status" role="status">
          <span className="small-dot live" />
          <span>正在整理 {activeJobs.length} 项内容</span>
          <small>完成后出现在这里</small>
        </div>
      )}
      <JobFailures api={api} jobs={home.jobs} onChanged={onChanged} />
      <div className="reading-toolbar">
        <div className="section-tabs" role="group" aria-label="阅读内容">
          <button
            aria-pressed={category === "research"}
            className={category === "research" ? "selected" : ""}
            onClick={() => onCategory("research")}
          >
            深度研究{research.length > 0 && <span>{research.length}</span>}
          </button>
          <button
            aria-pressed={category === "digest"}
            className={category === "digest" ? "selected" : ""}
            onClick={() => onCategory("digest")}
          >
            兴趣日报{digests.length > 0 && <span>{digests.length}</span>}
          </button>
        </div>
      </div>
      <div className="reading-update">
        <p>可随时更新，也可以重新同步已完成的内容。</p>
        <div className="inline-actions">
          <button
            className="secondary"
            disabled={busy}
            onClick={() => void onResearch(category)}
          >
            <Icon name="refresh" size={16} />
            {busy ? "提交中…" : category === "digest" ? "更新日报" : "更新研究"}
          </button>
          <button
            className="text-button"
            disabled={loading}
            onClick={onChanged}
          >
            同步内容
          </button>
        </div>
        <small>生成研究会使用额度，同步内容不消耗额度。</small>
      </div>
      <ReadingArchive
        api={api}
        timezone={home.profile.timezone}
        kind={category}
        refreshKey={home.reports.map((r) => `${r.id}:${r.status}`).join(",")}
        onReport={(id) => {
          onCategory(category);
          onReport(id);
        }}
        emptyAction={
          category === "research" ? () => navigate("records") : onPreferences
        }
      />
      <div className="schedule-note">
        <Icon name="clock" size={15} />
        <span>
          {String(home.profile.researchHour ?? 23).padStart(2, "0")}:00 开始研究
          · {String(home.profile.digestHour).padStart(2, "0")}:00 接收材料 ·{" "}
          {home.profile.timezone}
        </span>
      </div>
    </>
  );
}
