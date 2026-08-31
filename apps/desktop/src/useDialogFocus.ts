import { useEffect, useRef } from "react";

type DialogFocusOptions = {
  /** Called when the user presses Escape while the dialog owns focus. */
  onEscape?: () => void;
};

/**
 * Shared dialog focus controller: on activation the dialog receives initial
 * focus, Tab/Shift+Tab wrap inside it, Escape optionally cancels, and focus
 * returns to the previously focused element on deactivation. Covers the
 * alertdialog/modal gaps from the 2026-08-31 audit (P1-07).
 */
export function useDialogFocus<T extends HTMLElement>(
  active: boolean,
  options: DialogFocusOptions = {}
) {
  const ref = useRef<T | null>(null);
  const escapeRef = useRef(options.onEscape);
  escapeRef.current = options.onEscape;

  useEffect(() => {
    if (!active) return;
    const node = ref.current;
    if (!node) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (!node.hasAttribute("tabindex")) node.setAttribute("tabindex", "-1");
    node.focus({ preventScroll: true });

    const focusables = () =>
      Array.from(
        node.querySelectorAll<HTMLElement>(
          'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'
        )
      ).filter((element) => !element.hasAttribute("disabled"));

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && escapeRef.current) {
        event.stopPropagation();
        escapeRef.current();
        return;
      }
      if (event.key !== "Tab") return;
      const items = focusables();
      if (items.length === 0) {
        event.preventDefault();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const current = document.activeElement;
      if (event.shiftKey && (current === first || current === node)) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && current === last) {
        event.preventDefault();
        first.focus();
      }
    };

    node.addEventListener("keydown", onKeyDown);
    return () => {
      node.removeEventListener("keydown", onKeyDown);
      previous?.focus({ preventScroll: true });
    };
  }, [active]);

  return ref;
}
