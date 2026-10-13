const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const path = require("node:path");
const os = require("node:os");
const fs = require("node:fs");

test("normal installed desktop refuses a workstation before creating an account", {skip:process.platform!=="win32"},()=>{
  const result=run(["-File",path.join(__dirname,"with-installed-desktop.ps1"),"-Script",path.join(__dirname,"test-connected-upgrade.ps1")],{...process.env,GITHUB_ACTIONS:"false",RUNNER_OS:"Windows"});
  assert.notEqual(result.status,0);
  assert.match(result.stderr,/Disposable GitHub Windows runner required/);
});

test("installed desktop refuses an action outside this checkout before changing the shell", {skip:process.platform!=="win32"},t=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),"atlas-external-action-"));
  t.after(()=>fs.rmSync(root,{recursive:true,force:true}));
  const action=path.join(root,"action.ps1");fs.writeFileSync(action,"throw 'external action ran'");
  const result=run(["-File",path.join(__dirname,"with-installed-desktop.ps1"),"-Script",action],{...process.env,GITHUB_ACTIONS:"true",RUNNER_OS:"Windows"});
  assert.notEqual(result.status,0);
  assert.match(result.stderr,/Installed action must belong to this checkout/);
  assert.doesNotMatch(result.stderr,/external action ran/);
});

test("ordinary desktop action helper refuses a workstation before launching Atlas", {skip:process.platform!=="win32"},()=>{
  const result=run(["-File",path.join(__dirname,"invoke-installed-desktop-check.ps1"),"-Action","ManualRestart","-EvidencePath",path.join(os.tmpdir(),"unused-atlas-proof.log")],{...process.env,GITHUB_ACTIONS:"false",RUNNER_OS:"Windows"});
  assert.notEqual(result.status,0);assert.match(result.stderr,/Disposable GitHub Windows runner required/);
});

test("private desktop permission lease and cleanup refuse the workstation", {skip:process.platform!=="win32"},()=>{
  const lease=path.join(__dirname,"installed-desktop-lease.cs").replaceAll("'","''");
  const cleanup=path.join(__dirname,"installed-test-process.cs").replaceAll("'","''");
  // Use the same Windows PowerShell host as the other native declaration checks.
  const result=run(["-Command",`$ErrorActionPreference='Stop'; Write-Output 'Compile desktop lease'; Add-Type -Path '${lease}'; Write-Output 'Compile process cleanup'; Add-Type -Path '${cleanup}'; Write-Output 'Check workstation guards'; try {[AtlasInstalledDesktopLease]::new([Security.Principal.WindowsIdentity]::GetCurrent().User.Value);throw 'Lease accepted'} catch {if($_.Exception.ToString() -notmatch 'Disposable CI desktop required'){throw}}; try {[AtlasInstalledTestProcess]::StopPrivateShell([uint32]$PID,[Security.Principal.WindowsIdentity]::GetCurrent().User.Value);throw 'Cleanup accepted'} catch {if($_.Exception.ToString() -notmatch 'Disposable GitHub Windows runner required'){throw}}; Write-Output 'PASS guarded native fixtures'`],{...process.env,GITHUB_ACTIONS:"false"},"powershell.exe");
  assert.equal(result.status,0,result.stdout+result.stderr);
  assert.match(result.stdout,/PASS guarded native fixtures/);
});

function run(args, env = process.env, executable = "pwsh.exe") {
  const result = spawnSync(executable, ["-NoProfile", ...args], {
    encoding: "utf8",
    timeout: 20000,
    env,
  });
  assert.ifError(result.error && Object.assign(result.error, {
    message: `${executable}: ${result.error.message}\nstdout:\n${result.stdout || ""}\nstderr:\n${result.stderr || ""}`,
  }));
  return result;
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
    const result = run(
      [
        "-Command",
        `Add-Type -Path '${file}'; $level=[AtlasInstalledShell]::Integrity([uint32]$PID); if($level -notin 8192,12288,16384){throw 'Unexpected process integrity'}; try { [AtlasInstalledShell]::VerifyShell([uint32]$PID); throw 'Non-shell process accepted' } catch { if($_.Exception.ToString() -notmatch 'Refusing a non-Explorer process'){throw} }; $shell=[AtlasInstalledShell]::ShellPid(); if($shell){[AtlasInstalledShell]::VerifyShell($shell); if([AtlasInstalledShell]::ShellPid() -ne $shell){throw 'Read-only verification changed the shell'}}; [AtlasInstalledShell]::ShellDiagnostic(); [AtlasInstalledShell]::ReleaseShellObservation(); [AtlasInstalledShell]::ShellDiagnostic()`,
      ],
      process.env,
      "powershell.exe",
    );
    assert.equal(result.status, 0, result.stdout + result.stderr);
    const observations = result.stdout.trim().split(/\r?\n/);
    assert.equal(observations.length, 2);
    for (const observation of observations) {
      assert.match(observation, /^shell PID=\d+; launched shell=not observed; station=\S+; desktop=\S+; session=\d+$/);
      assert.doesNotMatch(observation, /unavailable:/);
    }
  },
);

test(
  "installed shell setup requires a scoped action before inspecting the desktop",
  { skip: process.platform !== "win32" },
  (t) => {
    const fs = require("node:fs");
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "atlas-shell-action-"));
    t.after(() => fs.rmSync(root, { recursive: true, force: true }));
    const result = run(
      ["-File", path.join(__dirname, "initialize-installed-shell.ps1"), "-InstallRoot", fs.realpathSync.native(root)],
      { ...process.env, GITHUB_ACTIONS: "true", RUNNER_OS: "Windows", RUNNER_TEMP: fs.realpathSync.native(root) },
    );
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Installed shell fixture requires a scoped test action/);
  },
);

test(
  "shell restart policy remains leased through the action and restores value, type and absence on success or failure",
  { skip: process.platform !== "win32" },
  () => {
    const helper = path.join(__dirname, "shell-restart-policy.ps1").replaceAll("'", "''");
    const result = run(
      ["-ExecutionPolicy", "Bypass", "-Command", `
$ErrorActionPreference='Stop'
. '${helper}'
$keyPath='Software\\Atlas-Shell-Lease-'+[Guid]::NewGuid().ToString('N')
if($keyPath -notmatch '^Software\\\\Atlas-Shell-Lease-[a-f0-9]{32}$'){throw 'Invalid disposable key'}
$key=[Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($keyPath)
try {
  foreach($kind in 'Absent','DWord','QWord','String') {
    foreach($fails in $false,$true) {
      $key.DeleteValue('AutoRestartShell',$false)
      if($kind -ne 'Absent') {
        $original=if($kind -eq 'String'){'original-policy'}else{17}
        $key.SetValue('AutoRestartShell',$original,[Microsoft.Win32.RegistryValueKind]::$kind)
      }
      $caught=$null
      try {
        $returned=Invoke-ShellRestartPolicyLease -Registry $key -Body {
          if($key.GetValue('AutoRestartShell') -ne 0 -or $key.GetValueKind('AutoRestartShell') -ne 'DWord'){throw 'Policy not leased inside action'}
          if($fails){throw 'fixture-body-failure'}
          'body-result'
        }
      } catch { $caught=$_ }
      if($fails) {
        if(-not $caught -or $caught.Exception.Message -ne 'fixture-body-failure'){throw 'Original action exception not propagated'}
      } elseif($caught -or $returned -ne 'body-result') {throw 'Action result not preserved'}
      if($kind -eq 'Absent') {
        if($key.GetValueNames() -contains 'AutoRestartShell'){throw 'Originally absent policy was retained'}
      } elseif($key.GetValue('AutoRestartShell') -ne $original -or $key.GetValueKind('AutoRestartShell').ToString() -ne $kind) {
        throw 'Original policy value or type not restored'
      }
    }
  }
} finally {
  $key.Dispose()
  [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($keyPath,$false)
}`],
      process.env,
      "powershell.exe",
    );
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stdout + result.stderr);
  },
);

test(
  "tray restart refuses a workstation before any UI action",
  { skip: process.platform !== "win32" },
  () => {
    const result = run(
      [
        "-File",
        path.join(__dirname, "invoke-installed-tray-restart.ps1"),
        "-ProcessId",
        String(process.pid),
        "-Name",
        "Перезагрузить",
      ],
      { ...process.env, GITHUB_ACTIONS: "false", RUNNER_OS: "Windows" },
    );
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Disposable GitHub Windows runner required/);
  },
);
test(
  "parent tray registration query refuses a workstation before inspecting its image or windows",
  { skip: process.platform !== "win32" },
  () => {
    const result = run(
      ["-File", path.join(__dirname, "get-installed-tray-registration.ps1"),
        "-ProcessId", String(process.pid), "-Executable", "absent-Atlas.exe",
        "-OutputPath", "absent-proof.json"],
      { ...process.env, GITHUB_ACTIONS: "false", RUNNER_OS: "Windows" },
    );
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Disposable GitHub Windows runner required/);
  },
);
test(
  "parent tray registration query rejects an image outside RUNNER_TEMP before native access",
  { skip: process.platform !== "win32" },
  () => {
    const result = run(
      ["-File", path.join(__dirname, "get-installed-tray-registration.ps1"),
        "-ProcessId", String(process.pid), "-Executable", process.execPath,
        "-OutputPath", path.join(os.tmpdir(), "unused-tray-proof.json")],
      { ...process.env, GITHUB_ACTIONS: "true", RUNNER_OS: "Windows", RUNNER_TEMP: os.tmpdir() },
    );
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Registration query requires the installed runner desktop image/);
  },
);
test(
  "registered-tray diagnostic declarations compile without touching a window",
  { skip: process.platform !== "win32" },
  () => {
    const source = require("node:fs").readFileSync(
      path.join(__dirname, "invoke-installed-tray-restart.ps1"),
      "utf8",
    );
    const code = /@'\r?\n([\s\S]*?)\r?\n'@/.exec(source)?.[1];
    assert(code, "Native tray diagnostic source required");
    const encoded = Buffer.from(code).toString("base64");
    const result = run(
      [
        "-Command",
        "Add-Type -TypeDefinition ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('" +
          encoded +
          "')))",
      ],
      process.env,
      "powershell.exe",
    );
    assert.equal(result.status, 0, result.stdout + result.stderr);
  },
);

test(
  "a detached tray helper preserves its early exception before closing the transcript",
  { skip: process.platform !== "win32" },
  (t) => {
    const fs = require("node:fs");
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "atlas-tray-error-"));
    t.after(() => fs.rmSync(root, { recursive: true, force: true }));
    const evidence = path.join(root, "negative.log");
    // PID 0 cannot own an Atlas window. The script must fail during process
    // inspection, before shell discovery, cursor movement or input delivery.
    const result = run(
      [
        "-ExecutionPolicy", "Bypass", "-File",
        path.join(__dirname, "invoke-installed-tray-restart.ps1"),
        "-ProcessId", "0", "-Name", "negative-test", "-EvidencePath", evidence,
      ],
      { ...process.env, GITHUB_ACTIONS: "true", RUNNER_OS: "Windows", RUNNER_TEMP: root },
      "powershell.exe",
    );
    assert.ifError(result.error);
    assert.equal(result.status, 1, result.stdout + result.stderr);
    for (const file of [evidence, evidence + ".error.txt"]) {
      const diagnostic = fs.readFileSync(file, "utf8");
      assert.match(diagnostic, /Inspect process \(Win32=87\)/);
      assert.match(diagnostic, /Script stack:.*invoke-installed-tray-restart\.ps1/);
    }
  },
);
