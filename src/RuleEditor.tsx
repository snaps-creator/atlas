import type { Route } from "./types";
export const routeName = (r: Route) =>
  r === "PROXY" ? "VPN" : r === "DIRECT" ? "Напрямую" : "Блокировать";
export function RouteSelect({
  value,
  onChange,
}: {
  value: Route;
  onChange: (r: Route) => void;
}) {
  return (
    <select
      aria-label="Маршрут"
      className={`route route-${value}`}
      value={value}
      onChange={(e) => onChange(e.target.value as Route)}
    >
      <option value="PROXY">VPN</option>
      <option value="DIRECT">Напрямую</option>
      <option value="BLOCK">Блокировать</option>
    </select>
  );
}
