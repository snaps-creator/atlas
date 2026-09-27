import { expect, test } from "vitest";
import { siteResultLabel } from "./siteChecks";

test("local contention, network failure and HTTP refusal remain distinct", () => {
  const failed = {ok:false,ms:null,status:null};
  expect(siteResultLabel({...failed,errorKind:"local"})).toBe("Проверка не выполнена");
  expect(siteResultLabel({...failed,errorKind:"timeout"})).toBe("Таймаут запроса");
  expect(siteResultLabel({...failed,errorKind:"network"})).toBe("Ошибка соединения");
  expect(siteResultLabel({...failed,status:403})).toBe("HTTP 403");
  expect(siteResultLabel({ok:true,status:200,ms:42})).toBe("HTTP-ответ: 42 мс");
});
