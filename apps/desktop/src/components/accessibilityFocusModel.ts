export type PermissionFocusTarget = "request" | "composer" | null;

export function permissionFocusTarget(
  previousRequestId: string | null,
  currentRequestId: string | null,
  restoreAfterDecision: boolean
): PermissionFocusTarget {
  if (currentRequestId && currentRequestId !== previousRequestId) return "request";
  if (!currentRequestId && previousRequestId && restoreAfterDecision) return "composer";
  return null;
}

export function wrappedDialogFocusIndex(
  currentIndex: number,
  itemCount: number,
  direction: 1 | -1
): number | null {
  if (itemCount <= 0) return null;
  const start = currentIndex >= 0 ? currentIndex : direction === 1 ? -1 : 0;
  return (start + direction + itemCount) % itemCount;
}
