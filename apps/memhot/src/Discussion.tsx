import { useEffect, useLayoutEffect, useRef, useState, type FormEvent } from "react";
import { Api, errorText } from "./api";
import { Markdown } from "./Markdown";
import { EvidenceList, Loading, Notice } from "./components";
import { Icon } from "./Icon";
import type { Message, Report, Thought } from "./types";
import { useBackAction } from "./useBackAction";
import { backLayer } from "./backNavigation";
export function Discussion({
  api,
  context,
  previousConversationId,
  onBack,
  onChanged,
  text,
  onText,
}: {
  api: Api;
  context: { report?: Report; thought?: Thought };
  previousConversationId?: string;
  onBack: () => void;
  onChanged: () => void;
  text: string;
  onText: (text: string) => void;
}) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [conversationId, setConversationId] = useState<string>(
    previousConversationId || "",
  );
  const [loading, setLoading] = useState(Boolean(previousConversationId));
  const [historyFailed, setHistoryFailed] = useState(false);
  const [reload, setReload] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const request = useRef<AbortController | null>(null);
  const operation = useRef<{ signature: string; key: string } | null>(null);
  const [retryConfirm, setRetryConfirm] = useState(false);
  useBackAction(retryConfirm, () => { if (!busy) setRetryConfirm(false); }, backLayer.editor);
  const end = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => {
    const field = input.current;
    if (!field) return;
    const resize = () => {
      field.style.height = "0px";
      field.style.height = `${Math.min(field.scrollHeight, 168)}px`;
      field.style.overflowY = field.scrollHeight > 168 ? "auto" : "hidden";
    };
    resize();
    // Reflow pasted text and drafts after rotation without reserving empty rows.
    let width = field.clientWidth;
    const observer = new ResizeObserver(() => {
      if (field.clientWidth !== width) { width = field.clientWidth; resize(); }
    });
    observer.observe(field);
    return () => observer.disconnect();
  }, [text]);
  const title = context.report?.title || context.thought?.title || "继续思考";
  // Resolve prior history once on entering a context. Home polling after send must not reset it.
  const initialConversation = useRef(previousConversationId);
  useEffect(() => {
    const key = initialConversation.current;
    if (!key) return;
    const controller = new AbortController();
    setLoading(true);
    setHistoryFailed(false);
    void api
      .request<{ id: string; messages: Message[] }>(
        `/api/v1/conversations/${encodeURIComponent(key)}`,
        "GET",
        undefined,
        controller.signal,
      )
      .then((data) => {
        if (!controller.signal.aborted) {
          setMessages(data.messages);
          setConversationId(data.id);
          setError("");
        }
      })
      .catch((e) => {
        if (!controller.signal.aborted) {
          setError(errorText(e));
          setHistoryFailed(true);
        }
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [api, reload]);
  useEffect(() => () => request.current?.abort(), []);
  useEffect(() => {
    end.current?.scrollIntoView({ behavior: "instant", block: "nearest" });
  }, [messages, busy]);
  async function send(event?: FormEvent, suggestion?: string) {
    event?.preventDefault();
    const question = (suggestion || text).trim();
    if (!question || busy || loading || historyFailed) return;
    if (new TextEncoder().encode(question).length > 12000) {
      setError("问题超过 12KB，请分成几轮讨论");
      return;
    }
    setBusy(true);
    setError("");
    const controller = new AbortController();
    request.current = controller;
    const signature = JSON.stringify([
      context.report?.id,
      context.thought?.id,
      conversationId,
      question,
    ]);
    if (operation.current?.signature !== signature)
      operation.current = { signature, key: crypto.randomUUID() };
    try {
      const result = await api.request<{
        conversationId: string;
        message: Message;
      }>(
        "/api/v1/discuss",
        "POST",
        {
          reportId: context.report?.id,
          thoughtId: context.thought?.id,
          conversationId: conversationId || undefined,
          message: question,
        },
        controller.signal,
        operation.current.key,
      );
      if (controller.signal.aborted) return;
      setConversationId(result.conversationId);
      setMessages((previous) => [
        ...previous,
        {
          id: crypto.randomUUID(),
          role: "user",
          text: question,
          createdAt: new Date().toISOString(),
        },
        result.message,
      ]);
      if (!suggestion) onText("");
      onChanged();
      operation.current = null;
      setRetryConfirm(false);
    } catch (e) {
      if (!controller.signal.aborted) setError(errorText(e));
    } finally {
      if (request.current === controller) {
        setBusy(false);
        request.current = null;
      }
    }
  }
  return (
    <section className="discussion">
      <button className="text-button back-button" onClick={onBack}>
        <Icon name="back" size={18} />
        {context.report ? "返回材料" : "返回思考"}
      </button>
      <header className="discussion-context">
        <Icon name="chat" size={23} />
        <div>
          <h1>沿着这个问题，接着聊</h1>
          <p>{title}</p>
        </div>
      </header>
      <div className="messages" aria-live="polite">
        {loading && <Loading />}
        {!loading && !historyFailed && messages.length === 0 && (
          <div className="discussion-welcome">
            <p>
              可以检视一个假设，找找不同观点，
              <br />
              也可以说说这些材料让你想到了什么。
            </p>
            <div className="suggestions">
              {[
                "这里最需要验证的假设是什么？",
                "有哪些材料可能推翻我的想法？",
                "如果继续探索，下一步可以做什么？",
              ].map((s) => (
                <button
                  key={s}
                  onClick={() => void send(undefined, s)}
                  disabled={busy}
                >
                  {s}
                  <Icon name="arrow" size={16} />
                </button>
              ))}
            </div>
          </div>
        )}
        {messages.map((message) => (
          <div className={`message ${message.role}`} key={message.id}>
            <span className="message-author">
              {message.role === "user" ? "你" : "Memhot"}
            </span>
            <Markdown text={message.text} />
            {message.sources?.length ? (
              <details className="message-sources">
                <summary>本轮引用材料 · {message.sources.length}</summary>
                <EvidenceList evidence={message.sources} />
              </details>
            ) : null}
          </div>
        ))}
        {busy && (
          <div className="thinking" role="status">
            <span className="small-dot live" />
            正在查阅资料，整理回答。需要搜索时会稍久一些…
            <button
              className="text-button"
              onClick={() => {
                request.current?.abort();
                setBusy(false);
              }}
            >
              取消
            </button>
          </div>
        )}
        <div ref={end} />
      </div>
      {error && (
        <Notice
          error={error}
          onRetry={historyFailed ? () => setReload((n) => n + 1) : undefined}
        />
      )}
      {error &&
        !historyFailed &&
        (retryConfirm ? (
          <div className="delete-confirm">
            <p>
              上次请求可能已处理。再次发送会发起新的研究请求，可能重复使用额度。
            </p>
            <button
              disabled={busy}
              className="secondary"
              onClick={() => {
                operation.current = null;
                setRetryConfirm(false);
                void send();
              }}
            >
              确认再次发送
            </button>
            <button
              className="text-button"
              onClick={() => setRetryConfirm(false)}
            >
              取消
            </button>
          </div>
        ) : (
          <button
            className="text-button"
            disabled={busy || !text.trim()}
            onClick={() => setRetryConfirm(true)}
          >
            作为新请求再次发送
          </button>
        ))}
      <form className="chat-composer" onSubmit={send}>
        <textarea
          ref={input}
          aria-label="继续讨论的问题"
          value={text}
          readOnly={busy}
          onChange={(e) => onText(e.target.value)}
          placeholder="说说你的想法…"
          rows={1}
          maxLength={10000}
        />
        <button
          className="primary icon-button"
          type="submit"
          aria-label="发送问题"
          disabled={busy || loading || historyFailed || !text.trim()}
        >
          <Icon name="arrowUp" size={21} />
        </button>
      </form>
      <p className="chat-footnote">
        会结合当前材料与可用检索。未找到证据的部分，应当保留为问题。
      </p>
    </section>
  );
}
