/**
 * Runs `work` once after the current task, however often the returned
 * function is called during it.
 *
 * Monaco can emit one content change per character inside a single input
 * event (Firefox commits a typed text as one composition, and Monaco types it
 * character by character), so work that reads the whole text on every change
 * costs the text's length squared. Deferred to a microtask, it runs once, and
 * still before the browser renders the next frame.
 */
export function oncePerTask(work: () => void): () => void {
  let queued = false;
  return () => {
    if (queued) return;
    queued = true;
    queueMicrotask(() => {
      queued = false;
      work();
    });
  };
}
