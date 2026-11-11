/** Schedules reads only. Financial actions are deliberately outside this helper. */
export function createCksolReadPolling(options: {
  read: () => Promise<void>;
  active: () => boolean;
  fingerprint: () => string;
  visible: () => boolean;
}) {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let running: Promise<void> | null = null;
  let destroyed = false;
  let armed = false;
  let previous = '';
  let delay = 15_000;
  function clear() { if (timer !== null) clearTimeout(timer); timer = null; }
  function sync() {
    if (destroyed || !options.active()) { clear(); armed = false; return; }
    if (!options.visible()) { clear(); return; }
    const next = options.fingerprint();
    if (!armed || (!running && next !== previous)) { clear(); armed = true; delay = 15_000; previous = next; }
    if (!running && timer === null) timer = setTimeout(() => { timer = null; void run(false); }, delay);
  }
  function run(manual: boolean): Promise<void> {
    if (destroyed) return Promise.resolve();
    if (running) return running;
    if (!manual && (!options.active() || !options.visible())) { sync(); return Promise.resolve(); }
    clear();
    // Start in a microtask so `running` is installed even for an immediate read.
    running = Promise.resolve().then(() => { if (!destroyed) return options.read(); }).finally(() => {
      const next = options.fingerprint();
      delay = manual || next !== previous ? 15_000 : Math.min(delay * 2, 60_000);
      previous = next;
      running = null;
      sync();
    });
    return running;
  }
  return { sync, checkAgain: () => run(true), destroy: () => { destroyed = true; clear(); } };
}
export type CksolReadPolling = ReturnType<typeof createCksolReadPolling>;
