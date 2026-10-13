const { test } = require("node:test"),
  assert = require("node:assert/strict"),
  fs = require("node:fs"),
  os = require("node:os"),
  path = require("node:path"),
  { spawnSync } = require("node:child_process");
test(
  "offline hive acceptance refuses a workstation before any registry change",
  { skip: process.platform !== "win32" },
  () => {
    const fixture = path.join(os.tmpdir(), "atlas-offline-guard-" + Date.now());
    const r = spawnSync(
      "pwsh",
      [
        "-NoProfile",
        "-File",
        "scripts/test-offline-startup-hive.ps1",
        "-Mode",
        "Setup",
        "-Fixture",
        fixture,
        "-InstallRoot",
        fixture,
      ],
      { encoding: "utf8", env: { ...process.env, GITHUB_ACTIONS: "false" } },
    );
    assert.ifError(r.error);
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /Disposable GitHub Windows runner required/);
    assert(!fs.existsSync(fixture));
  },
);
test(
  "offline hive acceptance refuses paths outside the disposable fixture",
  { skip: process.platform !== "win32" },
  () => {
    const fixture = path.join(os.tmpdir(), "atlas-offline-scope-" + Date.now());
    const r = spawnSync(
      "pwsh",
      [
        "-NoProfile",
        "-File",
        "scripts/test-offline-startup-hive.ps1",
        "-Mode",
        "Setup",
        "-Fixture",
        fixture,
        "-InstallRoot",
        fixture,
      ],
      {
        encoding: "utf8",
        env: {
          ...process.env,
          GITHUB_ACTIONS: "true",
          RUNNER_OS: "Windows",
          RUNNER_TEMP: path.join(fixture, "allowed"),
        },
      },
    );
    assert.ifError(r.error);
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /fixture must stay under RUNNER_TEMP/);
    assert(!fs.existsSync(fixture));
  },
);
