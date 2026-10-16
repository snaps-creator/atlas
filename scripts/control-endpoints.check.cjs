const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
test("3.0.3 display latency is distinct from HTTPS route controls and uses certified evidence", () => {
  const read = file => fs.readFileSync(path.join(__dirname,"..",file),"utf8");
  const backend=read("src-tauri/src/latency.rs");
  for (const control of ["TUNNEL_URL","SECONDARY_URL"]) {
    const endpoint=new RegExp('pub const '+control+': &str = "([^"]+)"').exec(backend)?.[1];
    assert(endpoint);assert.equal(new URL(endpoint).protocol,"https:");
  }
  assert.match(backend,/pub const DISPLAY_URL: &str = "http:\/\/cp\.cloudflare\.com\/generate_204"/);
  assert.match(backend,/pub const ENDPOINTS: \[&str; 2\] = \[TUNNEL_URL, SECONDARY_URL\]/);
  assert.match(read("src-tauri/src/core.rs"),/self\.tunnel_responds_at\(crate::latency::TUNNEL_URL, timeout\)/);
  assert.match(read("src-tauri/src/config.rs"),/doc\["unified-delay"\] = json!\(true\)/);
  assert.match(read("src/usePoolRecovery.ts"),/atlasHealth/);
  assert.match(read("src-tauri/src/latency_refresh.rs"),/overlay_live/);
});
