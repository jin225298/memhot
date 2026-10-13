import { Icon } from "./Icon";
import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "./ui/accordion";
import type { Report, Thought } from "./types";
import { groupThoughts } from "./thoughtArchive";

export function ThoughtTimeline({
  thoughts,
  reports,
  timezone,
  groupBy,
  expanded,
  onSelect,
  onArchive,
  onDelete,
  pending,
}: {
  thoughts: Thought[];
  reports: Report[];
  timezone: string;
  groupBy: "date" | "category";
  expanded: boolean;
  onSelect: (id: string) => void;
  onArchive: (thought: Thought) => void;
  onDelete: (thought: Thought) => void;
  pending: string;
}) {
  const groups = groupThoughts(thoughts, timezone, groupBy);
  return (
    <Accordion
      className="thought-timeline"
      type="multiple"
      defaultValue={
        expanded
          ? groups.map((g) => g.key)
          : groups.slice(0, 1).map((g) => g.key)
      }
    >
      {groups.map((group) => (
        <AccordionItem
          className="thought-collection"
          key={group.key}
          value={group.key}
        >
          <AccordionTrigger className="collection-heading">
            <span>
              <strong>{group.label}</strong>
              <small>
                {group.items.length} 个思考 ·{" "}
                {group.items
                  .slice(0, 2)
                  .map((t) => t.title)
                  .join("、")}
              </small>
            </span>
          </AccordionTrigger>
          <AccordionContent className="collection-items">
            {group.items.map((thought) => {
              const related = reports.filter(
                (r) => r.thoughtId === thought.id,
              ).length;
              const closed =
                thought.status === "dismissed" || thought.status === "resolved";
              return (
                <article className="thought-entry" key={thought.id}>
                  <button
                    className="thought-entry-open"
                    onClick={() => onSelect(thought.id)}
                  >
                    <div className="detail-meta">
                      <span>{thought.category || "未分类"}</span>
                      <span>
                        {thought.analysisStatus !== "ready"
                          ? "正在整理"
                          : related
                            ? `${related} 份研究`
                            : closed
                              ? "已归档"
                              : "待研究"}
                      </span>
                    </div>
                    <h3>{thought.title}</h3>
                    <p>{thought.summary || thought.question}</p>
                    <span className="thought-open-label">
                      查看思考 <Icon name="arrow" size={15} />
                    </span>
                  </button>
                  <div className="thought-entry-actions">
                    <button
                      className="text-button"
                      disabled={pending === thought.id}
                      onClick={() => onArchive(thought)}
                    >
                      <Icon name={closed ? "refresh" : "archive"} size={16} />
                      {pending === thought.id
                        ? "处理中…"
                        : closed
                          ? "继续思考"
                          : "归档"}
                    </button>
                    <button
                      className="text-button delete-thought-button"
                      disabled={pending === thought.id}
                      onClick={() => onDelete(thought)}
                    >
                      <Icon name="trash" size={16} />
                      删除
                    </button>
                  </div>
                </article>
              );
            })}
          </AccordionContent>
        </AccordionItem>
      ))}
    </Accordion>
  );
}
