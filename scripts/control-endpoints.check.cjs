const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
test("controls use HTTPS; desktop consumes certified evidence instead of raw per-URL history", () => {
  const read = file => fs.readFileSync(path.join(__dirname,"..",file),"utf8");
  const backend=read("src-tauri/src/latency.rs");
  for (const control of ["DISPLAY_URL","SECONDARY_URL"]) {
    const endpoint=new RegExp('pub const '+control+': &str = "([^"]+)"').exec(backend)?.[1];
    assert(endpoint);assert.equal(new URL(endpoint).protocol,"https:");
  }
  assert.match(read("src/usePoolRecovery.ts"),/atlasHealth/);
  assert.match(read("src-tauri/src/latency_refresh.rs"),/overlay_live/);
});
