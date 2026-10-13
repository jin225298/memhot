import { Icon } from "./Icon";

export function ReportContents({
  entries,
  label = "阅读目录",
}: {
  entries: { id: string; title: string }[];
  label?: string;
}) {
  if (entries.length < 2) return null;
  return (
    <details className="report-contents">
      <summary>
        <span>
          {label}
          <small>{entries.length} 节</small>
        </span>
        <Icon name="chevron" size={16} />
      </summary>
      <nav aria-label={label}>
        {entries.map((entry) => (
          <a key={entry.id} href={`#${entry.id}`}>
            {entry.title}
            <Icon name="arrow" size={15} />
          </a>
        ))}
      </nav>
    </details>
  );
}
