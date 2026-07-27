import { useCallback, useRef } from "react";

type PendingSelection<Value> = {
  operation: () => Promise<Value>;
  waiters: Array<{
    resolve: (value: Value) => void;
    reject: (error: unknown) => void;
  }>;
};

export function useLatestAsyncSelection<Value>() {
  const runningRef = useRef(false);
  const pendingRef = useRef<PendingSelection<Value> | null>(null);

  const drain = useCallback(async () => {
    if (runningRef.current) return;
    runningRef.current = true;
    try {
      while (pendingRef.current) {
        const pending = pendingRef.current;
        pendingRef.current = null;
        try {
          const value = await pending.operation();
          pending.waiters.forEach(({ resolve }) => resolve(value));
        } catch (error) {
          pending.waiters.forEach(({ reject }) => reject(error));
        }
      }
    } finally {
      runningRef.current = false;
    }
  }, []);

  return useCallback(
    (operation: () => Promise<Value>) => {
      const request = new Promise<Value>((resolve, reject) => {
        const pending = pendingRef.current;
        if (pending) {
          pending.operation = operation;
          pending.waiters.push({ resolve, reject });
        } else {
          pendingRef.current = {
            operation,
            waiters: [{ resolve, reject }]
          };
        }
      });
      void drain();
      return request;
    },
    [drain]
  );
}
