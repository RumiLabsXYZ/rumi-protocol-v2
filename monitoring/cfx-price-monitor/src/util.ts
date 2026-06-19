/**
 * Race a promise against a hard deadline. If `promise` does not settle within
 * `ms`, the returned promise rejects with a timeout error. This is the guard
 * that prevents a single hung network call from wedging the monitor loop (and,
 * with it, silently disabling the downtime watchdog).
 */
export function withTimeout<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${label} timed out after ${ms}ms`)), ms);
    promise.then(
      (v) => {
        clearTimeout(timer);
        resolve(v);
      },
      (e) => {
        clearTimeout(timer);
        reject(e instanceof Error ? e : new Error(String(e)));
      },
    );
  });
}
