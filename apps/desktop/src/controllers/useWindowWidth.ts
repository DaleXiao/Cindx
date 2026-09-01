import { useEffect, useState } from "react";

/**
 * Live window inner width for window-aware layout contracts (the panel
 * constraint that keeps the thread column usable at narrow widths).
 */
export function useWindowWidth() {
  const [windowWidth, setWindowWidth] = useState(() =>
    typeof window === "undefined" ? 1280 : window.innerWidth
  );
  useEffect(() => {
    const update = () => setWindowWidth(window.innerWidth);
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);
  return windowWidth;
}
