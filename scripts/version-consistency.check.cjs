const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const root = path.join(__dirname, "..");
const read = (p) => fs.readFileSync(path.join(root, p), "utf8");
const json = (p) => JSON.parse(read(p));
test("product manifests agree and candidate is newer than published 2.4.2", () => {
  const version = json("package.json").version;
  assert.match(version, /^\d+\.\d+\.\d+$/);
  const parts = version.split(".").map(Number);
  const baseline = [2, 4, 2];
  const difference = parts.map((v, i) => v - baseline[i]).find((v) => v !== 0);
  assert.ok(difference > 0, "candidate must be newer than 2.4.2");
  const lock = json("package-lock.json");
  for (const value of [
    lock.version,
    lock.packages[""].version,
    json("src-tauri/tauri.conf.json").version,
    json("updater-readiness.json").version,
    read("src-tauri/Cargo.toml").match(/^version = "([^"]+)"/m)[1],
    read("src-tauri/Cargo.lock").match(
      /name = "atlas-vpn"\r?\nversion = "([^"]+)"/,
    )[1],
  ]) {
    assert.equal(value, version);
  }
});
