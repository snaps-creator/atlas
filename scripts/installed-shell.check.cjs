const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const path = require("node:path");
const os = require("node:os");

function run(args, env = process.env) {
  return spawnSync("pwsh.exe", ["-NoProfile", ...args], {
    encoding: "utf8",
    timeout: 20000,
    env,
  });
}

test(
  "installed shell setup refuses the user's host before inspecting it",
  { skip: process.platform !== "win32" },
  () => {
    const result = run(
      [
        "-File",
        path.join(__dirname, "initialize-installed-shell.ps1"),
        "-InstallRoot",
        process.cwd(),
      ],
      {
        ...process.env,
        GITHUB_ACTIONS: "false",
        RUNNER_OS: "Windows",
      },
    );
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Disposable GitHub Windows runner required/);
  },
);

test(
  "installed shell setup refuses a root outside the disposable fixture",
  { skip: process.platform !== "win32" },
  () => {
    const result = run(
      [
        "-File",
        path.join(__dirname, "initialize-installed-shell.ps1"),
        "-InstallRoot",
        process.cwd(),
      ],
      {
        ...process.env,
        GITHUB_ACTIONS: "true",
        RUNNER_OS: "Windows",
        RUNNER_TEMP: os.tmpdir(),
      },
    );
    assert.notEqual(result.status, 0);
    assert.match(
      result.stderr,
      /Shell fixture requires an installation under RUNNER_TEMP/,
    );
  },
);

test(
  "shell native declarations compile and inspect only the current token",
  { skip: process.platform !== "win32" },
  () => {
    const file = path
      .join(__dirname, "installed-shell.cs")
      .replaceAll("'", "''");
    const result = run([
      "-Command",
      `Add-Type -Path '${file}'; $level=[AtlasInstalledShell]::Integrity([uint32]$PID); if($level -notin 8192,12288,16384){throw 'Unexpected process integrity'}`,
    ]);
    assert.equal(result.status, 0, result.stdout + result.stderr);
  },
);
