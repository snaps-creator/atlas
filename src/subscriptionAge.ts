export function subscriptionAge(updatedSeconds: number, nowMs: number): string {
  const minutes = Math.floor(Math.max(0, nowMs / 1000 - updatedSeconds) / 60);
  if (minutes < 1) return "только что";
  if (minutes < 60) return `${minutes} мин назад`;
  if (minutes < 1440) return `${Math.floor(minutes / 60)} ч назад`;
  return `${Math.floor(minutes / 1440)} дн назад`;
}
