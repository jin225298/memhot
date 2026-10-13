import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { safeExternalUrl } from "./api";
import { ExternalLink } from "./ExternalLink";

export function Markdown({ text }: { text: string }) {
  return (
    <div className="prose">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ href, children }) => {
            const safe = safeExternalUrl(href || "");
            return safe ? (
              <ExternalLink href={safe}>{children}</ExternalLink>
            ) : (
              <span>{children}</span>
            );
          },
          img: ({ alt }) => (
            <span>{alt ? `[图片：${alt}]` : "[图片请到原文查看]"}</span>
          ),
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
