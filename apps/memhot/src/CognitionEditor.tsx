import { useRef, useState, type FormEvent } from "react";
import { Api, errorText } from "./api";
import { Notice } from "./components";
import { SelectField } from "./ui/select";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from "./ui/dialog";
import type { Cognition as Item } from "./types";
import { useBackAction } from "./useBackAction";
import { backLayer } from "./backNavigation";

export const cognitionLabels: Record<Item["type"], string> = {
  interest: "兴趣方向",
  preference: "阅读与思考偏好",
  goal: "正在追求的目标",
  belief: "观点与判断",
  constraint: "需要考虑的条件",
};
export function CognitionEditor({
  api,
  item,
  onBack,
  onSaved,
  initialType = "interest",
}: {
  api: Api;
  item?: Item;
  initialType?: Item["type"];
  onBack: () => void;
  onSaved: () => void;
}) {
  const opener = useRef(document.activeElement as HTMLElement | null);
  const [statement, setStatement] = useState(item?.statement || "");
  const [type, setType] = useState<Item["type"]>(item?.type || initialType);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  useBackAction(true, () => {
    if (!busy) { if (confirmDelete) setConfirmDelete(false); else onBack(); }
  }, backLayer.editor);
  async function save(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api.request(
        item
          ? `/api/v1/cognition/${encodeURIComponent(item.id)}`
          : "/api/v1/cognition",
        item ? "PUT" : "POST",
        {
          statement: statement.trim(),
          type,
          ...(item ? { revision: item.revision, status: "active" } : {}),
        },
      );
      onSaved();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  async function remove() {
    if (!item) return;
    setBusy(true);
    setError("");
    try {
      await api.request(
        `/api/v1/cognition/${encodeURIComponent(item.id)}`,
        "DELETE",
      );
      onSaved();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onBack();
      }}
    >
      <DialogContent
        closeDisabled={busy}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          opener.current?.focus();
        }}
        onPointerDownOutside={(event) => event.preventDefault()}
        onEscapeKeyDown={(event) => {
          if (busy) event.preventDefault();
        }}
      >
        <DialogTitle>{item ? "编辑偏好" : "添加偏好"}</DialogTitle>
        <DialogDescription className="field-hint">
          写下你的习惯、目标，或修正 AI 的理解。
        </DialogDescription>
        <section className="settings-page cognition-editor">
          <form onSubmit={save}>
            <label>
              分类
              <SelectField
                label="偏好分类"
                value={type}
                onValueChange={(value) => setType(value as Item["type"])}
                options={Object.entries(cognitionLabels).map(
                  ([value, label]) => ({ value, label }),
                )}
                disabled={busy}
              />
            </label>
            <label>
              内容
              <textarea
                autoFocus
                readOnly={busy}
                required
                minLength={2}
                maxLength={2000}
                rows={5}
                value={statement}
                onChange={(e) => setStatement(e.target.value)}
                placeholder="例如：我想了解 AI 产品能否形成真实的付费需求，喜欢有数据和反例的材料。"
              />
            </label>
            <p className="field-hint">
              你填写的内容会成为明确的个性化依据。之后的自动总结不会悄悄覆盖它。
            </p>
            {error && <Notice error={error} />}
            <button className="primary" disabled={busy || !statement.trim()}>
              {busy ? "保存中…" : "保存"}
            </button>
          </form>
          {item &&
            (confirmDelete ? (
              <div className="delete-confirm">
                <p>
                  删除这条偏好及其内容？系统会保留最小去重标记，避免自动总结再次加回来。
                </p>
                <button
                  disabled={busy}
                  className="danger-text"
                  onClick={() => void remove()}
                >
                  确认删除
                </button>
                <button disabled={busy} onClick={() => setConfirmDelete(false)}>
                  取消
                </button>
              </div>
            ) : (
              <button
                className="text-button danger-text"
                onClick={() => setConfirmDelete(true)}
              >
                删除这条偏好
              </button>
            ))}
        </section>
      </DialogContent>
    </Dialog>
  );
}
