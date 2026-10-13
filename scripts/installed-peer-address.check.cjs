const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const path = require("node:path");

function select(fixture) {
  const script = path
    .join(__dirname, "select-installed-peer-address.ps1")
    .replaceAll("'", "''");
  return spawnSync(
    "pwsh.exe",
    [
      "-NoProfile",
      "-Command",
      `${fixture}
function Get-NetIPConfiguration { throw 'Ambiguous NetAdapter join' }
function Get-NetAdapter { throw 'Adapter join must not be used' }
& '${script}'`,
    ],
    { encoding: "utf8", timeout: 15000 },
  );
}

test(
  "peer uses the lowest metric usable route without the ambiguous adapter join",
  { skip: process.platform !== "win32" },
  () => {
    const result = select(`
function Get-NetRoute {
  [pscustomobject]@{InterfaceIndex=8;RouteMetric=1;InterfaceMetric=100}
  [pscustomobject]@{InterfaceIndex=9;RouteMetric=5;InterfaceMetric=10}
}
function Get-NetIPAddress {
  param($AddressFamily,$InterfaceIndex)
  if($AddressFamily -ne 'IPv4'){throw 'Wrong address family'}
  if($InterfaceIndex -eq 9){
    [pscustomobject]@{IPAddress='192.0.2.9';AddressState='Preferred';SkipAsSource=$false}
    [pscustomobject]@{IPAddress='192.0.2.10';AddressState='Preferred';SkipAsSource=$false}
  } else {throw 'Wrong default route selected'}
}`);
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.equal(result.stdout.trim(), "192.0.2.9");
  },
);

test(
  "peer skips tentative, excluded, loopback and link-local addresses",
  { skip: process.platform !== "win32" },
  () => {
    const result = select(`
function Get-NetRoute {
  [pscustomobject]@{InterfaceIndex=8;RouteMetric=1;InterfaceMetric=1}
  [pscustomobject]@{InterfaceIndex=9;RouteMetric=2;InterfaceMetric=1}
}
function Get-NetIPAddress {
  param($AddressFamily,$InterfaceIndex)
  if($InterfaceIndex -eq 8){
    [pscustomobject]@{IPAddress='192.0.2.8';AddressState='Tentative';SkipAsSource=$false}
    [pscustomobject]@{IPAddress='192.0.2.10';AddressState='Preferred';SkipAsSource=$true}
    [pscustomobject]@{IPAddress='169.254.1.1';AddressState='Preferred';SkipAsSource=$false}
    [pscustomobject]@{IPAddress='127.0.0.1';AddressState='Preferred';SkipAsSource=$false}
  } else {
    [pscustomobject]@{IPAddress='192.0.2.9';AddressState='Preferred';SkipAsSource=$false}
  }
}`);
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.equal(result.stdout.trim(), "192.0.2.9");
  },
);

test(
  "peer fails explicitly when there is no IPv4 default route",
  { skip: process.platform !== "win32" },
  () => {
    const result = select("function Get-NetRoute {} ");
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /No usable default-route IPv4 address/);
  },
);
