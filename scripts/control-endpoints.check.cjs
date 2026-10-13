const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

test("desktop observations read the same per-URL health records as backend controls", () => {
  const root = path.join(__dirname, "..");
  const backend = fs.readFileSync(path.join(root, "src-tauri/src/latency.rs"), "utf8");
  const desktop = fs.readFileSync(path.join(root, "src/usePoolRecovery.ts"), "utf8");
  for (const [rust, ts] of [["DISPLAY_URL", "PRIMARY"], ["SECONDARY_URL", "SECONDARY"]]) {
    const actual = new RegExp(`pub const ${rust}: &str = "([^"]+)"`).exec(backend)?.[1];
    const observed = new RegExp(`const ${ts} = "([^"]+)"`).exec(desktop)?.[1];
    assert(actual, `Backend ${rust} control required`);
    assert.equal(observed, actual, `${ts} must read the backend's health key`);
  }
});
