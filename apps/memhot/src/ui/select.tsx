// Adapted from shadcn/ui (MIT), new-york-v4/ui/select.tsx.
// Utility classes are mapped to Memhot tokens; Radix owns focus and selection.
import { useState, type ComponentProps } from "react";
import { useBackAction } from "../useBackAction";
import { backLayer } from "../backNavigation";
import * as SelectPrimitive from "@radix-ui/react-select";
import { Icon } from "../Icon";

export const Select = SelectPrimitive.Root;
export const SelectValue = SelectPrimitive.Value;
export function SelectTrigger({
  children,
  className = "",
  ...props
}: ComponentProps<typeof SelectPrimitive.Trigger>) {
  return (
    <SelectPrimitive.Trigger
      className={`ui-select-trigger ${className}`}
      {...props}
    >
      {children}
      <SelectPrimitive.Icon asChild>
        <span className="ui-chevron">
          <Icon name="chevron" size={16} />
        </span>
      </SelectPrimitive.Icon>
    </SelectPrimitive.Trigger>
  );
}
export function SelectContent({
  children,
  ...props
}: ComponentProps<typeof SelectPrimitive.Content>) {
  return (
    <SelectPrimitive.Portal>
      <SelectPrimitive.Content
        className="ui-select-content"
        position="popper"
        sideOffset={6}
        collisionPadding={12}
        {...props}
      >
        <SelectPrimitive.ScrollUpButton className="ui-select-scroll">
          <span className="ui-chevron up">
            <Icon name="chevron" size={15} />
          </span>
        </SelectPrimitive.ScrollUpButton>
        <SelectPrimitive.Viewport className="ui-select-viewport">
          {children}
        </SelectPrimitive.Viewport>
        <SelectPrimitive.ScrollDownButton className="ui-select-scroll">
          <span className="ui-chevron">
            <Icon name="chevron" size={15} />
          </span>
        </SelectPrimitive.ScrollDownButton>
      </SelectPrimitive.Content>
    </SelectPrimitive.Portal>
  );
}
export function SelectItem({
  children,
  ...props
}: ComponentProps<typeof SelectPrimitive.Item>) {
  return (
    <SelectPrimitive.Item className="ui-select-item" {...props}>
      <SelectPrimitive.ItemText>{children}</SelectPrimitive.ItemText>
      <SelectPrimitive.ItemIndicator className="ui-select-indicator">
        <Icon name="check" size={16} />
      </SelectPrimitive.ItemIndicator>
    </SelectPrimitive.Item>
  );
}

export function SelectField({
  value,
  onValueChange,
  options,
  label,
  disabled,
  id,
}: {
  value: string;
  onValueChange: (value: string) => void;
  options: { value: string; label: string }[];
  label: string;
  disabled?: boolean;
  id?: string;
}) {
  const [open, setOpen] = useState(false);
  useBackAction(open, () => setOpen(false), backLayer.menu);
  return (
    <Select value={value} onValueChange={onValueChange} disabled={disabled} open={open} onOpenChange={setOpen}>
      <SelectTrigger aria-label={label} id={id}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {options.map((option) => (
          <SelectItem key={option.value} value={option.value}>
            {option.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
