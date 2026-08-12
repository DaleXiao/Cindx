export interface ComposerTextareaSizing {
  heightPx: number;
  overflowY: "auto" | "hidden";
}

export function composerTextareaSizing(
  scrollHeight: number,
  minHeightPx: number,
  maxHeightPx: number
): ComposerTextareaSizing {
  const safeScroll = Math.max(0, scrollHeight);
  const heightPx = Math.min(maxHeightPx, Math.max(minHeightPx, safeScroll));
  return {
    heightPx,
    overflowY: safeScroll > maxHeightPx ? "auto" : "hidden"
  };
}
