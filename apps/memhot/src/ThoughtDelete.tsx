import { useState } from "react";
import { Api, errorText } from "./api";
import { Notice } from "./components";
import type { Thought } from "./types";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "./ui/dialog";
import { useBackAction } from "./useBackAction";
import { backLayer } from "./backNavigation";

export function ThoughtDelete({
  api,
  thought,
  onClose,
  onDeleted,
}: {
  api: Api;
  thought: Thought;
  onClose: () => void;
  onDeleted: (less: boolean) => void;
}) {
  const [less, setLess] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useBackAction(
    true,
    () => {
      if (!busy) onClose();
    },
    backLayer.editor,
  );
  async function remove() {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const query = new URLSearchParams({ preference: less ? "less" : "none" });
      if (thought.revision != null)
        query.set("revision", String(thought.revision));
      await api.request(
        `/api/v1/thoughts/${encodeURIComponent(thought.id)}?${query}`,
        "DELETE",
      );
      onDeleted(less);
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
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent
        className="ui-dialog-content thought-delete-dialog"
        closeDisabled={busy}
        closeLabel="取消删除"
      >
        <DialogTitle>删除这个思考？</DialogTitle>
        <p className="delete-thought-title">{thought.title}</p>
        <DialogDescription className="field-hint">
          这个思考及其研究、讨论将被删除，原始记录会保留。如果只是暂时放下，可以取消后归档。
        </DialogDescription>
        <label className="preference-feedback">
          <input
            type="checkbox"
            checked={less}
            disabled={busy}
            onChange={(e) => setLess(e.target.checked)}
          />
          <span>
            以后少推荐类似问题
            <small>
              保存到个人偏好，可以随时修改。仅删除不会被理解为不感兴趣。
            </small>
          </span>
        </label>
        {error && <Notice error={error} />}
        <div className="delete-dialog-actions">
          <button
            className="secondary"
            disabled={busy}
            onClick={onClose}
            autoFocus
          >
            取消
          </button>
          <button
            className="primary delete-action"
            disabled={busy}
            onClick={() => void remove()}
          >
            {busy ? "删除中…" : "删除思考"}
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
