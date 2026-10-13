// Adapted from shadcn/ui (MIT), new-york-v4/ui/dialog.tsx.
import type { ComponentProps } from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { Icon } from "../Icon";

export const Dialog = DialogPrimitive.Root;
export const DialogTitle = DialogPrimitive.Title;
export const DialogDescription = DialogPrimitive.Description;
export function DialogContent({
  children,
  closeDisabled,
  closeLabel = "关闭编辑",
  ...props
}: ComponentProps<typeof DialogPrimitive.Content> & {
  closeDisabled?: boolean;
  closeLabel?: string;
}) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay className="ui-dialog-overlay" />
      <DialogPrimitive.Content className="ui-dialog-content" {...props}>
        {children}
        <DialogPrimitive.Close
          className="ui-dialog-close icon-button"
          disabled={closeDisabled}
          aria-label={closeLabel}
        >
          <Icon name="close" size={18} />
        </DialogPrimitive.Close>
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}
