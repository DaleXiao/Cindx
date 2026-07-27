import { useCallback, useEffect, useMemo, useState } from "react";
import {
  IGNORED_PERMISSION_REVIEWS_STORAGE_KEY,
  loadIgnoredPermissionReviewIds
} from "../appShellModel";
import { getPermissionReviewState, type PermissionReviewItem } from "../tauri";

function sameIds(left: Set<string>, right: Set<string>) {
  return left.size === right.size && [...left].every((id) => right.has(id));
}

function sameReviews(left: PermissionReviewItem[], right: PermissionReviewItem[]) {
  return (
    left.length === right.length &&
    left.every((review, index) => {
      const candidate = right[index];
      return (
        candidate?.requestId === review.requestId &&
        candidate.action === review.action &&
        candidate.risk === review.risk &&
        candidate.reason === review.reason &&
        candidate.scope === review.scope &&
        candidate.source === review.source &&
        candidate.projectId === review.projectId &&
        candidate.projectName === review.projectName &&
        candidate.sessionId === review.sessionId &&
        candidate.sessionName === review.sessionName &&
        candidate.input === review.input &&
        candidate.requestedAtMs === review.requestedAtMs &&
        candidate.canAllowSession === review.canAllowSession
      );
    })
  );
}

function persistIgnoredIds(ids: Set<string>) {
  try {
    window.localStorage.setItem(
      IGNORED_PERMISSION_REVIEWS_STORAGE_KEY,
      JSON.stringify([...ids])
    );
  } catch {
    // Keep the preference for this app session when storage is unavailable.
  }
}

export function usePermissionReviewController(pollingEnabled: boolean) {
  const [reviews, setReviews] = useState<PermissionReviewItem[]>([]);
  const [ignoredIds, setIgnoredIds] = useState(loadIgnoredPermissionReviewIds);

  const refresh = useCallback(async () => {
    const next = await getPermissionReviewState();
    setReviews((current) => (sameReviews(current, next.pending) ? current : next.pending));
    return next;
  }, []);

  useEffect(() => {
    if (!pollingEnabled) return;
    let disposed = false;
    let inFlight = false;
    const poll = () => {
      if (disposed || inFlight) return;
      inFlight = true;
      void getPermissionReviewState()
        .then((next) => {
          if (!disposed) {
            setReviews((current) =>
              sameReviews(current, next.pending) ? current : next.pending
            );
          }
        })
        .finally(() => {
          inFlight = false;
        });
    };
    poll();
    const timer = window.setInterval(poll, 2_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [pollingEnabled]);

  useEffect(() => {
    const pendingIds = new Set(reviews.map((review) => review.requestId));
    setIgnoredIds((current) => {
      const next = new Set([...current].filter((requestId) => pendingIds.has(requestId)));
      if (sameIds(current, next)) return current;
      persistIgnoredIds(next);
      return next;
    });
  }, [reviews]);

  const updateIgnored = useCallback((requestId: string, ignored: boolean) => {
    setIgnoredIds((current) => {
      const next = new Set(current);
      if (ignored) next.add(requestId);
      else next.delete(requestId);
      if (sameIds(current, next)) return current;
      persistIgnoredIds(next);
      return next;
    });
  }, []);

  const activeReviews = useMemo(
    () => reviews.filter((review) => !ignoredIds.has(review.requestId)),
    [ignoredIds, reviews]
  );
  const ignoredReviews = useMemo(
    () => reviews.filter((review) => ignoredIds.has(review.requestId)),
    [ignoredIds, reviews]
  );

  return {
    activeReviews,
    ignoreReview: (requestId: string) => updateIgnored(requestId, true),
    ignoredReviews,
    refresh,
    restoreReview: (requestId: string) => updateIgnored(requestId, false)
  };
}
