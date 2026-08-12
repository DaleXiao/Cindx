import { useEffect, useRef, useState } from "react";
import { Terminal } from "lucide-react";
import type { CustomCommandView, CustomCommandsState } from "../tauriTypes";
import { getCustomCommands } from "../tauri";

type CustomCommandsMenuProps = {
  disabled?: boolean;
  onApply: (command: CustomCommandView) => void;
};

export function CustomCommandsMenu({ disabled, onApply }: CustomCommandsMenuProps) {
  const [state, setState] = useState<CustomCommandsState | null>(null);
  const [open, setOpen] = useState(false);
  const controlRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    let cancelled = false;
    void getCustomCommands().then((next) => {
      if (!cancelled) setState(next);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!open) return;
    const closeOnPointerDown = (event: globalThis.PointerEvent) => {
      if (!controlRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setOpen(false);
    };
    document.addEventListener("pointerdown", closeOnPointerDown);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnPointerDown);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);

  const commands = state?.commands ?? [];
  if (commands.length === 0) {
    return null;
  }

  return (
    <div className="composer-commands-control" data-open={open} ref={controlRef}>
      <button
        className="composer-commands-trigger"
        type="button"
        aria-label="Custom commands"
        aria-haspopup="listbox"
        aria-expanded={open}
        title="Custom commands"
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
      >
        <Terminal aria-hidden="true" />
      </button>
      {open && (
        <div className="composer-commands-menu" role="listbox" aria-label="Custom commands">
          {commands.map((command) => (
            <button
              type="button"
              role="option"
              aria-selected={false}
              key={`${command.scope}:${command.name}`}
              onClick={() => {
                setOpen(false);
                onApply(command);
              }}
            >
              <span>
                <strong>/{command.name}</strong>
                {command.description ? <small>{command.description}</small> : null}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
