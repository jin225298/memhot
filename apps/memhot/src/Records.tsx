import { useEffect, useRef, useState, type FormEvent } from "react";
import { Api, errorText, shortDate } from "./api";
import { Empty, Loading, Notice } from "./components";
import {
  Accordion,
  AccordionItem,
  AccordionTrigger,
  AccordionContent,
} from "./ui/accordion";
import { SelectField } from "./ui/select";
import { RecordingPanel } from "./RecordingPanel";
import { Icon } from "./Icon";
import type { CaptureStatus, Source } from "./types";
import { useBackAction } from "./useBackAction";
import { backLayer } from "./backNavigation";
const labels: Record<string, string> = {
  queued: "待处理",
  awaiting_audio: "等待设备同步录音",
  transcribing: "正在转写",
  ready: "已保存",
  failed: "处理失败",
  no_speech: "无有效语音 · 已跳过",
};
const analysisLabels: Record<string, string> = {
  pending: "等待分析",
  ready: "已分析",
  roles_required: "需要确认说话人",
  model_unconfigured: "待配置文字模型",
  failed: "分析未完成",
};
export function Records({
  api,
  sources,
  capture,
  recordBusy,
  onRecord,
  onChanged,
  onThoughts,
  active,
  selectedId,
  onSelect,
}: {
  api: Api;
  sources: Source[];
  capture: CaptureStatus;
  recordBusy: boolean;
  onRecord: () => Promise<void>;
  onChanged: () => void;
  onThoughts: () => void;
  active: boolean;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
}) {
  const [text, setText] = useState("");
  const [title, setTitle] = useState("");
  const [fileName, setFileName] = useState("");
  const [speakerMode, setSpeakerMode] = useState("roles");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<Source | null>(null);
  const [deleteId, setDeleteId] = useState("");
  const [sourceReload, setSourceReload] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const draftId = useRef(crypto.randomUUID());
  useBackAction(active && Boolean(deleteId), () => { if (!busy) setDeleteId(""); }, backLayer.editor);
  useEffect(() => {
    setSelected(null);
    setError("");
    if (!selectedId) return;
    const controller = new AbortController();
    void api
      .request<Source>(
        `/api/v1/sources/${encodeURIComponent(selectedId)}`,
        "GET",
        undefined,
        controller.signal,
      )
      .then((source) => {
        if (!controller.signal.aborted) setSelected(source);
      })
      .catch((e) => {
        if (!controller.signal.aborted) setError(errorText(e));
      });
    return () => controller.abort();
  }, [api, selectedId, sourceReload]);
  async function analyze() {
    if (!selectedId) return;
    setBusy(true);
    setError("");
    try {
      await api.request(
        `/api/v1/sources/${encodeURIComponent(selectedId)}/analyze`,
        "POST",
        {},
      );
      setNotice("已安排重新整理，将提取多个话题与潜在问题。");
      setSourceReload((n) => n + 1);
      onChanged();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  async function chooseFile(file?: File) {
    if (!file) return;
    setError("");
    if (file.size > 200000) {
      setError("请导入 200KB 以内的文本文件，较长对话可以按天拆分");
      return;
    }
    if (!/\.(txt|md|json|csv)$/i.test(file.name)) {
      setError(
        "当前支持 TXT、Markdown、JSON 和 CSV；PDF 或 Word 请先导出为文本",
      );
      return;
    }
    try {
      const content = await file.text();
      if (content.includes("\u0000"))
        throw new Error("这个文件似乎不是文本，请重新导出为 UTF-8");
      setText(content);
      setTitle(file.name.replace(/\.[^.]+$/, ""));
      setFileName(file.name);
      draftId.current = crypto.randomUUID();
    } catch (e) {
      setError(errorText(e));
    }
  }
  async function save(event: FormEvent) {
    event.preventDefault();
    if (!text.trim()) return;
    if (new TextEncoder().encode(text.trim()).length > 200000) {
      setError("这段内容超过 200KB，请按天或主题拆分后保存");
      return;
    }
    setBusy(true);
    setError("");
    try {
      await api.request("/api/v1/sources", "POST", {
        title: title.trim() || "随手记下的想法",
        text: text.trim(),
        kind: fileName ? "import" : "note",
        speakerMode: fileName ? speakerMode : "self",
        occurredAt: new Date().toISOString(),
        clientId: draftId.current,
      });
      setText("");
      setTitle("");
      setFileName("");
      draftId.current = crypto.randomUUID();
      setNotice("已保存，正在整理话题。");
      onChanged();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  async function remove(id: string) {
    setBusy(true);
    try {
      await api.request(`/api/v1/sources/${encodeURIComponent(id)}`, "DELETE");
      setDeleteId("");
      onSelect(null);
      onChanged();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  if (selectedId)
    return (
      <section className="source-reader">
        <button
          className="text-button back-button"
          onClick={() => onSelect(null)}
        >
          <Icon name="back" size={18} />
          返回录音与记录
        </button>
        {error && (
          <Notice error={error} onRetry={() => setSourceReload((n) => n + 1)} />
        )}
        {notice && (
          <p role="status" className="success-message">
            {notice}
          </p>
        )}
        {!selected ? (
          !error && <Loading />
        ) : (
          <>
            <div className="section-heading">
              <h2>{selected.title}</h2>
              <button
                className="icon-button"
                aria-label="关闭原文"
                onClick={() => onSelect(null)}
              >
                <Icon name="close" />
              </button>
            </div>
            {selected.error && <Notice error={selected.error} />}
            {selected.analysisMessage && (
              <p className="muted">{selected.analysisMessage}</p>
            )}
            {selected.analysisCoverage && (
              <p className="field-hint">
                已读取{" "}
                {selected.analysisCoverage.includedChars.toLocaleString()} /{" "}
                {selected.analysisCoverage.totalChars.toLocaleString()} 字 ·{" "}
                {selected.analysisCoverage.complete
                  ? "全文已处理，话题仍可补充"
                  : "尚有内容待处理"}
              </p>
            )}
            <div className="inline-actions">
              <button className="text-button" onClick={onThoughts}>
                查看提取的思考
                <Icon name="arrow" size={16} />
              </button>
              <button
                className="text-button"
                disabled={busy || !selected.text}
                onClick={() => void analyze()}
              >
                {busy ? "提交中…" : "重新整理话题"}
              </button>
              <button
                className="icon-button"
                aria-label="刷新这条记录"
                onClick={() => setSourceReload((n) => n + 1)}
              >
                <Icon name="refresh" size={16} />
              </button>
            </div>
            <pre>
              {selected.text || "还没有可读取的文字。完成转写后请刷新。"}
            </pre>
          </>
        )}
      </section>
    );
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>记录此刻</h1>
          <p className="muted">聊过的事，想到的事，都可以从这里开始。</p>
        </div>
      </div>
      {error && <Notice error={error} />}
      <RecordingPanel
        active={active}
        capture={capture}
        recordBusy={recordBusy}
        onRecord={onRecord}
        onError={setError}
      />
      <Accordion type="single" collapsible className="supplementary-input">
        <AccordionItem value="import">
          <AccordionTrigger>
            <Icon name="upload" size={18} />
            <span>导入对话 / 写一段</span>
          </AccordionTrigger>
          <AccordionContent>
            <form className="note-composer" onSubmit={save}>
              <div className="section-heading">
                <span className="composer-label">此刻在想什么？</span>
                <button
                  type="button"
                  disabled={busy}
                  className="text-button"
                  onClick={() => input.current?.click()}
                >
                  <Icon name="upload" size={17} />
                  导入对话
                </button>
              </div>
              <input
                ref={input}
                type="file"
                accept=".txt,.md,.json,.csv,text/plain,application/json"
                className="sr-only"
                onChange={(e) => {
                  void chooseFile(e.target.files?.[0]);
                  e.target.value = "";
                }}
              />
              <details
                className="note-title-details"
                open={Boolean(fileName) || undefined}
              >
                <summary>{title || "添加标题（选填）"}</summary>
                <input
                  className="note-title"
                  aria-label="记录标题（选填）"
                  value={title}
                  readOnly={busy}
                  onChange={(e) => setTitle(e.target.value)}
                  placeholder="给它起个名字（选填）"
                  maxLength={160}
                />
              </details>
              <textarea
                aria-label="想法或对话原文"
                value={text}
                readOnly={busy}
                onChange={(e) => {
                  setText(e.target.value);
                  if (notice) setNotice("");
                }}
                placeholder="比如：我在想这个行业有没有机会，但还不知道谁会愿意付费……"
                rows={3}
                maxLength={200000}
              />
              {fileName && (
                <label className="import-speaker">
                  这份文件的内容
                  <SelectField
                    label="文件内容的说话人"
                    value={speakerMode}
                    disabled={busy}
                    onValueChange={setSpeakerMode}
                    options={[
                      { value: "roles", label: "AI 对话 · 按角色识别" },
                      { value: "unknown", label: "多人对话 · 身份待确认" },
                      { value: "self", label: "自己的原话或笔记" },
                    ]}
                  />
                  <small>
                    只有明确的本人发言才归入你的观点；会议可选择身份未确认，AI
                    对话请保留 user / assistant 角色。
                  </small>
                </label>
              )}
              <div className="composer-footer">
                <span>
                  {fileName ? (
                    <>
                      <Icon name="file" size={15} />
                      {fileName}
                    </>
                  ) : (
                    "随时补充"
                  )}
                </span>
                <button className="primary" disabled={busy || !text.trim()}>
                  {busy ? "保存中…" : "保存"}
                  <Icon name="arrow" size={17} />
                </button>
              </div>
            </form>
          </AccordionContent>
        </AccordionItem>
      </Accordion>
      {notice && (
        <p className="success-message" role="status">
          <Icon name="check" size={17} />
          {notice}
          <button className="text-button" onClick={onThoughts}>
            查看思考
            <Icon name="arrow" size={15} />
          </button>
        </p>
      )}
      <div className="section-heading">
        <h2>最近记录</h2>
        <span className="muted">{sources.length} 条</span>
      </div>
      {sources.length === 0 ? (
        <Empty icon="file" title="还没有记录">
          录音或导入后，可以在这里查看文字。
        </Empty>
      ) : (
        <div className="source-list">
          {sources.map((source) => (
            <div className="source-row" key={source.id}>
              <button onClick={() => onSelect(source.id)}>
                <Icon
                  name={source.kind === "audio" ? "mic" : "file"}
                  size={21}
                />
                <span>
                  <strong>{source.title}</strong>
                  <small>
                    {shortDate(source.createdAt)} ·{" "}
                    {labels[source.status] || source.status}
                    {source.analysisStatus &&
                      ` · ${analysisLabels[source.analysisStatus] || source.analysisStatus}`}
                  </small>
                </span>
                <Icon name="chevron" size={17} />
              </button>
              {deleteId === source.id ? (
                <div className="delete-confirm">
                  <span>删除原文并撤销相关记忆？</span>
                  <button
                    disabled={busy}
                    className="danger-text"
                    onClick={() => void remove(source.id)}
                  >
                    确认删除
                  </button>
                  <button onClick={() => setDeleteId("")}>取消</button>
                </div>
              ) : (
                <button
                  className="icon-button delete-source"
                  aria-label={`删除${source.title}`}
                  onClick={() => setDeleteId(source.id)}
                >
                  <Icon name="trash" size={17} />
                </button>
              )}
            </div>
          ))}
        </div>
      )}
    </>
  );
}
