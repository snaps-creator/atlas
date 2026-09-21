export type TrafficSample = { time: number; up: number; down: number };

// Totals are cumulative; use elapsed wall time rather than the polling interval.
export function trafficRate(
  previous: TrafficSample | null,
  current: TrafficSample,
): TrafficSample | null {
  if (!previous || current.time <= previous.time) return null;
  const seconds = (current.time - previous.time) / 1000;
  return {
    time: current.time,
    up: Math.max(0, current.up - previous.up) / seconds,
    down: Math.max(0, current.down - previous.down) / seconds,
  };
}
