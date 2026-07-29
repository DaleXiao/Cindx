import { useId } from "react";

type ProviderModelInputProps = {
  label: string;
  value: string;
  options: string[];
  emptyLabel?: string;
  disabled?: boolean;
  onChange: (value: string) => void;
};

export function ProviderModelInput({
  label,
  value,
  options,
  emptyLabel,
  disabled = false,
  onChange
}: ProviderModelInputProps) {
  const listId = useId();
  return (
    <label>
      <span>{label}</span>
      <input
        value={value}
        list={options.length > 0 ? listId : undefined}
        placeholder={emptyLabel}
        disabled={disabled}
        spellCheck={false}
        onChange={(event) => onChange(event.target.value)}
      />
      {options.length > 0 && (
        <datalist id={listId}>
          {options.map((model) => (
            <option value={model} key={model} />
          ))}
        </datalist>
      )}
    </label>
  );
}
