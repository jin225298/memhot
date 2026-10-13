// shadcn/ui Toggle Group composition (MIT), with the app's outline style.
import * as ToggleGroup from "@radix-ui/react-toggle-group";
import { Icon } from "../Icon";

export function ChoiceGroup({
  value,
  onValueChange,
  choices,
  label,
  disabled,
}: {
  value: string[];
  onValueChange: (value: string[]) => void;
  choices: { id: string; label: string }[];
  label: string;
  disabled?: boolean;
}) {
  return (
    <ToggleGroup.Root
      type="multiple"
      className="ui-choice-group"
      value={value}
      onValueChange={onValueChange}
      aria-label={label}
      disabled={disabled}
    >
      {choices.map((choice) => (
        <ToggleGroup.Item
          className="ui-choice"
          key={choice.id}
          value={choice.id}
          aria-label={choice.label}
        >
          {choice.label}
          <span className="choice-indicator" aria-hidden="true">
            {value.includes(choice.id) && <Icon name="check" size={14} />}
          </span>
        </ToggleGroup.Item>
      ))}
    </ToggleGroup.Root>
  );
}
