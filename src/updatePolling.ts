export const UPDATE_CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

export function startUpdatePolling<T>(
  check: () => Promise<T | null>,
  onUpdate: (update: T) => void,
  onError: (error: unknown) => void,
  events: { onStart?: () => void; onCurrent?: () => void } = {},
) {
  let active = true;
  let running = false;
  const inspect = async () => {
    if (!active || running) return;
    running = true;
    try {
      events.onStart?.();
      const update = await check();
      if (active && update !== null) {
        stop(); // Keep the offered update; do not replace it while downloading.
        onUpdate(update);
      } else if (active) {
        events.onCurrent?.();
      }
    } catch (error) {
      if (active) onError(error);
    } finally {
      running = false;
    }
  };
  // Deferring one tick lets React StrictMode cancel its first effect cleanly.
  const initial = setTimeout(() => void inspect(), 0);
  const interval = setInterval(() => void inspect(), UPDATE_CHECK_INTERVAL_MS);
  function stop() {
    active = false;
    clearTimeout(initial);
    clearInterval(interval);
  }
  return Object.assign(stop, { checkNow: inspect });
}
