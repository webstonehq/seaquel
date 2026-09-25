/**
 * Debounces an async job and keeps only the newest result.
 *
 * `schedule(run)` (re)starts the delay; when it fires, `run()` is called and
 * its result goes to `onResult`, unless a later `schedule` or `cancel`
 * happened in the meantime. A request counter guards the reply, so a slow
 * response can't overwrite a newer one. Errors from the newest request go to
 * `onError`; stale ones are dropped.
 */
export interface LatestDebounced<T> {
  schedule(run: () => Promise<T>): void;
  /** Drops the pending timer and any reply still in flight. */
  cancel(): void;
}

export function latestDebounced<T>(
  delayMs: number,
  onResult: (value: T) => void,
  onError: (error: unknown) => void = () => {},
): LatestDebounced<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let latest = 0;

  function cancel(): void {
    latest++;
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  }

  return {
    schedule(run) {
      cancel();
      const request = latest;
      timer = setTimeout(() => {
        timer = undefined;
        void (async () => {
          try {
            const value = await run();
            if (request === latest) onResult(value);
          } catch (error) {
            if (request === latest) onError(error);
          }
        })();
      }, delayMs);
    },
    cancel,
  };
}
