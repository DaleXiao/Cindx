import { useEffect, useState } from "react";

export type ResolvedTheme = "light" | "dark";

function readResolvedTheme(): ResolvedTheme {
  return document.documentElement.dataset.theme === "dark" ? "dark" : "light";
}

export function useResolvedTheme() {
  const [theme, setTheme] = useState<ResolvedTheme>(readResolvedTheme);

  useEffect(() => {
    const root = document.documentElement;
    const update = () => setTheme(readResolvedTheme());
    const observer = new MutationObserver(update);
    observer.observe(root, { attributes: true, attributeFilter: ["data-theme"] });
    update();
    return () => observer.disconnect();
  }, []);

  return theme;
}
