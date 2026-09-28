import { expect, test } from "vitest";
import { subscriptionAge } from "./subscriptionAge";
test("relative age changes without refreshing the subscription and tolerates clock skew", () => {
  expect(subscriptionAge(1000, 1000000)).toBe("только что");
  expect(subscriptionAge(1000, (1000 + 32 * 60) * 1000)).toBe("32 мин назад");
  expect(subscriptionAge(1000, (1000 + 7200) * 1000)).toBe("2 ч назад");
  expect(subscriptionAge(1000, (1000 + 86400) * 1000)).toBe("1 дн назад");
  expect(subscriptionAge(2000, 1000000)).toBe("только что");
});
