import { CheckCircle2, ChevronDown } from "lucide-react";
import {
  useEffect,
  useId,
  useRef,
  useState,
  type FocusEvent,
  type KeyboardEvent
} from "react";
import {
  filterProviderModelOptions,
  moveProviderModelOptionIndex,
  validProviderModelOptionIndex
} from "./providerModelInputModel";

type ProviderModelInputProps = {
  label: string;
  value: string;
  options: string[];
  emptyLabel?: string;
  disabled?: boolean;
  status?: {
    available: boolean;
    label: string;
    title?: string;
  };
  onChange: (value: string) => void;
};

export function ProviderModelInput({
  label,
  value,
  options,
  emptyLabel,
  disabled = false,
  status,
  onChange
}: ProviderModelInputProps) {
  const inputId = useId();
  const listId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  const [filterText, setFilterText] = useState("");
  const [activeIndex, setActiveIndex] = useState(-1);
  const visibleOptions = filterProviderModelOptions(options, filterText);
  const optionsIdentity = options.join("\u0000");
  const canOpen = !disabled && options.length > 0;

  useEffect(() => {
    if (!open) return;
    const handlePointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
        setActiveIndex(-1);
      }
    };
    document.addEventListener("pointerdown", handlePointerDown);
    return () => document.removeEventListener("pointerdown", handlePointerDown);
  }, [open]);

  useEffect(() => {
    setActiveIndex(-1);
  }, [optionsIdentity]);

  useEffect(() => {
    setActiveIndex((current) =>
      validProviderModelOptionIndex(current, visibleOptions.length)
    );
  }, [visibleOptions.length]);

  useEffect(() => {
    if (!canOpen) setOpen(false);
  }, [canOpen]);

  const selectOption = (model: string) => {
    onChange(model);
    setFilterText("");
    setOpen(false);
    setActiveIndex(-1);
    inputRef.current?.focus();
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (!canOpen) return;
      event.preventDefault();
      if (!open) {
        setFilterText("");
        setOpen(true);
        setActiveIndex(event.key === "ArrowDown" ? 0 : options.length - 1);
        return;
      }
      setActiveIndex((current) =>
        moveProviderModelOptionIndex(
          current,
          visibleOptions.length,
          event.key === "ArrowDown" ? "next" : "previous"
        )
      );
      return;
    }
    if (event.key === "Enter" && open && activeIndex >= 0) {
      const option = visibleOptions[activeIndex];
      if (option) {
        event.preventDefault();
        selectOption(option);
      }
      return;
    }
    if (event.key === "Escape" && open) {
      event.preventDefault();
      setOpen(false);
      setActiveIndex(-1);
    }
  };

  const handleBlur = (event: FocusEvent<HTMLDivElement>) => {
    if (event.currentTarget.contains(event.relatedTarget as Node | null)) return;
    setOpen(false);
    setActiveIndex(-1);
  };

  return (
    <div className="provider-model-field">
      <label htmlFor={inputId}>
        <span>{label}</span>
      </label>
      <div className={`provider-model-control-row${status ? " has-status" : ""}`}>
        <div className="provider-model-combobox" ref={rootRef} onBlur={handleBlur}>
          <input
            ref={inputRef}
            id={inputId}
            role="combobox"
            aria-autocomplete="list"
            aria-controls={listId}
            aria-expanded={open}
            aria-activedescendant={
              open && activeIndex >= 0 ? `${listId}-option-${activeIndex}` : undefined
            }
            value={value}
            placeholder={emptyLabel}
            disabled={disabled}
            spellCheck={false}
            onChange={(event) => {
              const nextValue = event.target.value;
              onChange(nextValue);
              setFilterText(nextValue);
              setActiveIndex(-1);
              setOpen(canOpen);
            }}
            onKeyDown={handleKeyDown}
          />
          <button
            className="provider-model-toggle"
            type="button"
            disabled={!canOpen}
            aria-label={`Show ${label} models`}
            aria-controls={listId}
            aria-expanded={open}
            onClick={() => {
              if (!canOpen) return;
              setFilterText("");
              setActiveIndex(-1);
              setOpen((current) => !current);
              inputRef.current?.focus();
            }}
          >
            <ChevronDown aria-hidden="true" />
          </button>
          {open && (
            <div className="provider-model-options" id={listId} role="listbox">
              {visibleOptions.length > 0 ? (
                visibleOptions.map((model, index) => (
                  <button
                    className={index === activeIndex ? "active" : undefined}
                    id={`${listId}-option-${index}`}
                    type="button"
                    role="option"
                    tabIndex={-1}
                    aria-selected={model === value}
                    key={model}
                    onMouseDown={(event) => event.preventDefault()}
                    onMouseEnter={() => setActiveIndex(index)}
                    onClick={() => selectOption(model)}
                  >
                    {model}
                  </button>
                ))
              ) : (
                <p>No matching models — keep typing to use a custom ID</p>
              )}
            </div>
          )}
        </div>
        {status &&
          (status.available ? (
            <span
              className="provider-model-check-wrap"
              title={status.title ?? status.label}
            >
              <CheckCircle2 className="provider-model-check" aria-label={status.label} />
            </span>
          ) : (
            <span className="provider-model-status-placeholder" aria-hidden="true" />
          ))}
      </div>
    </div>
  );
}
