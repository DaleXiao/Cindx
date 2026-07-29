export function filterProviderModelOptions(options: string[], query: string) {
  const normalizedQuery = query.trim().toLocaleLowerCase();
  if (!normalizedQuery) return options;
  return options.filter((option) => option.toLocaleLowerCase().includes(normalizedQuery));
}

export function moveProviderModelOptionIndex(
  currentIndex: number,
  optionCount: number,
  direction: "next" | "previous"
) {
  if (optionCount === 0) return -1;
  if (currentIndex < 0) return direction === "next" ? 0 : optionCount - 1;
  return direction === "next"
    ? (currentIndex + 1) % optionCount
    : (currentIndex - 1 + optionCount) % optionCount;
}

export function validProviderModelOptionIndex(currentIndex: number, optionCount: number) {
  return currentIndex >= 0 && currentIndex < optionCount ? currentIndex : -1;
}
