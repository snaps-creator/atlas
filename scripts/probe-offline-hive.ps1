$ErrorActionPreference='Stop'
if($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows'){throw 'Disposable Windows runner required'}
$fixture=Join-Path $env:RUNNER_TEMP ('offline-startup-'+[guid]::NewGuid().ToString('N'))
$root=Join-Path $env:RUNNER_TEMP 'registry-only-install'
./scripts/test-offline-startup-hive.ps1 -Mode Setup -Fixture $fixture -InstallRoot $root
try {
    & ./scripts/offline-hive-probe/target/debug/atlas-offline-hive-probe.exe $root
    if($LASTEXITCODE -ne 0){throw 'Native hive probe failed'}
    ./scripts/test-offline-startup-hive.ps1 -Mode Verify -Fixture $fixture -InstallRoot $root
} finally {./scripts/test-offline-startup-hive.ps1 -Mode Cleanup -Fixture $fixture -InstallRoot $root}
