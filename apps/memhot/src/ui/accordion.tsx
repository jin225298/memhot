// Adapted from shadcn/ui (MIT), new-york-v4/ui/accordion.tsx.
import type { ComponentProps } from "react";
import * as AccordionPrimitive from "@radix-ui/react-accordion";
import { Icon } from "../Icon";

export const Accordion = AccordionPrimitive.Root;
export function AccordionItem({
  className = "",
  ...props
}: ComponentProps<typeof AccordionPrimitive.Item>) {
  return (
    <AccordionPrimitive.Item
      className={`ui-accordion-item ${className}`}
      {...props}
    />
  );
}
export function AccordionTrigger({
  children,
  className = "",
  ...props
}: ComponentProps<typeof AccordionPrimitive.Trigger>) {
  return (
    <AccordionPrimitive.Header className="ui-accordion-header">
      <AccordionPrimitive.Trigger
        className={`ui-accordion-trigger ${className}`}
        {...props}
      >
        {children}
        <span className="ui-chevron">
          <Icon name="chevron" size={17} />
        </span>
      </AccordionPrimitive.Trigger>
    </AccordionPrimitive.Header>
  );
}
export function AccordionContent({
  children,
  className = "",
  ...props
}: ComponentProps<typeof AccordionPrimitive.Content>) {
  return (
    <AccordionPrimitive.Content className="ui-accordion-content" {...props}>
      <div className={className}>{children}</div>
    </AccordionPrimitive.Content>
  );
}
