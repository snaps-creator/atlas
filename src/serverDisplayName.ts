// Internal names remain unchanged for selection, latency and favorites.
export function serverDisplayName(name: string): string {
  return name.replace(/\s+·\s+[a-f0-9]{8}-\d+$/i, "");
}
