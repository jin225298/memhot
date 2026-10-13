import { useState, type MouseEvent, type ReactNode } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { safeExternalUrl } from "./api";

/** External articles never navigate the trusted native WebView itself. */
export function ExternalLink({
  href,
  children,
  className,
}: {
  href: string;
  children: ReactNode;
  className?: string;
}) {
  const [error, setError] = useState(false);
  const safe = safeExternalUrl(href);
  if (!safe) return <span>{children}</span>;
  async function open(event: MouseEvent<HTMLAnchorElement>) {
    if (!isTauri() || !safe) return;
    event.preventDefault();
    try {
      await openUrl(safe);
      setError(false);
    } catch {
      setError(true);
    }
  }
  return (
    <>
      <a
        href={safe}
        className={className}
        target="_blank"
        rel="noopener noreferrer"
        onClick={(e) => void open(e)}
      >
        {children}
      </a>
      {error && (
        <span role="alert" className="link-error">
          {" "}
          无法打开浏览器，请稍后重试或长按复制链接。
        </span>
      )}
    </>
  );
}
