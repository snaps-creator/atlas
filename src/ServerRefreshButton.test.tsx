import { expect, test, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ServerRefreshButton } from "./ServerRefreshButton";

test("a running background check leaves the restart action enabled", () => {
  const onRefresh=vi.fn();
  const button=ServerRefreshButton({count:169,checking:true,onRefresh});
  expect(button.props.disabled).toBe(false);
  button.props.onClick();button.props.onClick();
  expect(onRefresh).toHaveBeenCalledTimes(2);
  const html=renderToStaticMarkup(button);
  expect(html).toContain("Проверить заново");
  expect(html).not.toContain("disabled=");
});

test("only an empty catalogue disables the action", () => {
  for (const checking of [false,true]) {
    expect(ServerRefreshButton({count:0,checking,onRefresh:()=>{}}).props.disabled).toBe(true);
    expect(ServerRefreshButton({count:1,checking,onRefresh:()=>{}}).props.disabled).toBe(false);
  }
});
