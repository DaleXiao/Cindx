import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent
} from "react";
import { clampSidebarWidth } from "../appShellModel";

export function useSidebarResize(initialWidth = 236) {
  const [width, setWidth] = useState(initialWidth);
  const [resizing, setResizing] = useState(false);
  const cleanupRef = useRef<(() => void) | null>(null);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      cleanupRef.current?.();
    };
  }, []);

  const beginResize = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      event.preventDefault();
      cleanupRef.current?.();
      const startX = event.clientX;
      const startWidth = width;
      const previousCursor = document.body.style.cursor;
      const previousUserSelect = document.body.style.userSelect;
      setResizing(true);
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";

      const handleMove = (moveEvent: PointerEvent) => {
        setWidth(clampSidebarWidth(startWidth + moveEvent.clientX - startX));
      };
      const cleanup = () => {
        window.removeEventListener("pointermove", handleMove);
        window.removeEventListener("pointerup", cleanup);
        window.removeEventListener("pointercancel", cleanup);
        document.body.style.cursor = previousCursor;
        document.body.style.userSelect = previousUserSelect;
        if (mountedRef.current) setResizing(false);
        cleanupRef.current = null;
      };

      cleanupRef.current = cleanup;
      window.addEventListener("pointermove", handleMove);
      window.addEventListener("pointerup", cleanup);
      window.addEventListener("pointercancel", cleanup);
    },
    [width]
  );

  const handleResizeKeyDown = useCallback((event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "ArrowLeft") {
      event.preventDefault();
      setWidth((current) => clampSidebarWidth(current - 16));
    }
    if (event.key === "ArrowRight") {
      event.preventDefault();
      setWidth((current) => clampSidebarWidth(current + 16));
    }
  }, []);

  return {
    beginResize,
    handleResizeKeyDown,
    resizing,
    width
  };
}
