import { useState } from "react";
import { Icon } from "./Icon";
import { SelectField } from "./ui/select";
import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "./ui/accordion";
import type { DailySummary, Report, Thought } from "./types";

export function ThoughtTimeline({
  thoughts,
  days,
  reports,
  timezone,
  onSelect,
  onReport,
}: {
  thoughts: Thought[];
  days: DailySummary[];
  reports: Report[];
  timezone: string;
  onSelect: (id: string) => void;
  onReport: (id: string) => void;
}) {
  const [date, setDate] = useState("");
  const orderedDays = [...days].sort((a, b) =>
    b.localDate.localeCompare(a.localDate),
  );
  const entries = thoughts.map((thought) => {
    const day = orderedDays.find((d) =>
      d.sections.some((s) => s.topicId === thought.id),
    );
    const section = day?.sections.find((s) => s.topicId === thought.id);
    const localDate =
      day?.localDate ||
      new Intl.DateTimeFormat("sv-SE", {
        timeZone: timezone || "Asia/Shanghai",
      }).format(new Date(thought.createdAt));
    return { thought, day, section, localDate };
  });
  const dates = [...new Set(entries.map((entry) => entry.localDate))]
    .sort()
    .reverse();
  const visibleDates = date && dates.includes(date) ? [date] : dates;
  return (
    <div className="thought-timeline">
      <div className="timeline-toolbar">
        <SelectField
          label="筛选日期"
          value={dates.includes(date) ? date : "all"}
          onValueChange={(value) => setDate(value === "all" ? "" : value)}
          options={[
            { value: "all", label: "全部日期" },
            ...dates.map((day) => ({
              value: day,
              label: day.replaceAll("-", "."),
            })),
          ]}
        />
        <span>{thoughts.length} 个话题</span>
      </div>
      {visibleDates.map((localDate) => {
        const group = entries.filter((entry) => entry.localDate === localDate);
        return (
          <section className="thought-day" key={localDate}>
            <div className="thought-day-heading">
              <h2>{localDate.replaceAll("-", ".")}</h2>
              <span>{group.length} 个话题</span>
            </div>
            {group.some(
              (entry) => entry.day && !entry.day.coverage.complete,
            ) && <p className="field-hint">这一天还有记录正在整理。</p>}
            <Accordion type="multiple">
              {group.map(({ thought, section }) => {
                const related = reports.filter(
                  (report) => report.thoughtId === thought.id,
                );
                const summary = section?.summary || thought.summary;
                return (
                  <AccordionItem
                    className="topic-fold"
                    key={thought.id}
                    value={thought.id}
                  >
                    <AccordionTrigger>
                      <span className="topic-preview">
                        <span className="topic-title">{thought.title}</span>
                        {summary && (
                          <span className="topic-excerpt">{summary}</span>
                        )}
                        <span className="topic-meta">
                          {thought.analysisStatus !== "ready"
                            ? "正在整理"
                            : related.length
                              ? `${related.length} 份材料`
                              : "待研究"}
                          {thought.status === "kept" ? " · 已保留" : ""}
                        </span>
                      </span>
                    </AccordionTrigger>
                    <AccordionContent className="topic-unfolded">
                      {summary && <p className="topic-summary">{summary}</p>}
                      <h4>可以继续想的问题</h4>
                      <p className="topic-question">
                        {thought.analysisStatus === "ready"
                          ? thought.question
                          : "文字整理完成后，问题会出现在这里。"}
                      </p>
                      <div className="inline-actions">
                        <button
                          className="text-button"
                          onClick={() => onSelect(thought.id)}
                        >
                          展开思考 <Icon name="arrow" size={16} />
                        </button>
                        {related.length > 0 && (
                          <button
                            className="text-button"
                            onClick={() => onReport(related[0].id)}
                          >
                            <Icon name="book" size={16} />
                            阅读研究
                          </button>
                        )}
                      </div>
                    </AccordionContent>
                  </AccordionItem>
                );
              })}
            </Accordion>
          </section>
        );
      })}
    </div>
  );
}
