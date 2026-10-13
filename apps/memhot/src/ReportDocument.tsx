import { useEffect, useId, useRef, useState } from "react";
import { Markdown } from "./Markdown";
import { ReportContents } from "./ReportContents";

// Build the outline from rendered Markdown headings, so fenced code, escaped
// headings and setext headings follow the same parser as the document itself.
export function ReportDocument({ text }: { text: string }) {
  const root = useRef<HTMLDivElement>(null);
  const prefix = useId().replace(/:/g, "");
  const [entries, setEntries] = useState<{ id: string; title: string }[]>([]);
  useEffect(() => {
    const headings = Array.from(
      root.current?.querySelectorAll("h1, h2, h3") || [],
    );
    setEntries(
      headings.map((heading, i) => {
        const id = `report-${prefix}-${i}`;
        heading.id = id;
        return { id, title: heading.textContent || "章节" };
      }),
    );
  }, [text, prefix]);
  return (
    <>
      <ReportContents entries={entries} />
      <div className="report-document" ref={root}>
        <Markdown text={text} />
      </div>
    </>
  );
}
