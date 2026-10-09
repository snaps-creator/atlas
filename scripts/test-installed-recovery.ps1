param([Parameter(Mandatory=$true)][string]$InstallRoot, [Parameter(Mandatory=$true)][string]$Maintenance)
$ErrorActionPreference = 'Stop'
function Write-InterruptedRecoveryFixture([string]$Root, $Journal) {
    $Journal.stage = 'HealthPending'
    $Journal.health_state = 'unconfirmed'
    $Journal.sequence += 1
    $pending = Join-Path $Root 'recovery-fixture.json'
    $Journal | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $pending -Encoding utf8NoBOM
    # PowerShell binds $null to an empty string for this .NET string parameter.
    # Preserve an explicit backup instead of passing an invalid empty path.
    [IO.File]::Replace($pending,(Join-Path $Root 'current.json'),(Join-Path $Root 'recovery-before.json'))
}
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Installed recovery requires a disposable GitHub Windows runner' }
$root = (Resolve-Path -LiteralPath $InstallRoot).Path
$allowed = [IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\') + '\'
if (-not $root.StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase)) { throw 'Recovery fixture must remain under RUNNER_TEMP' }
$journalPath = Join-Path $root 'current.json'
$journal = Get-Content -LiteralPath $journalPath -Raw | ConvertFrom-Json
if ($journal.stage -ne 'Committed' -or $journal.previous.absent) { throw 'Recovery needs a committed upgrade with a retained previous version' }
$previous = $journal.previous.id
$previousRoot = Join-Path $root ('versions/' + $previous)
$candidateRoot = Join-Path $root ('versions/' + $journal.candidate.id)
$service = Get-CimInstance Win32_Service -Filter "Name='AtlasNetworkService'"
if ($service.PathName -ne ('"' + (Join-Path $candidateRoot 'Atlas.Service.exe') + '" --network-service')) { throw 'Refusing recovery of a service outside this candidate fixture' }
# A same-version installer returns after launching the desktop. Its setup holds
# the production update lease briefly; recovery must begin after that hand-off,
# not race it with a zero-timeout maintenance request. Never bypass the lease.
function Wait-RecoveryStartup {
$lease = $null
try { $lease = [Threading.Mutex]::OpenExisting('Global\Atlas.Update.Transaction.v1') }
catch [Threading.WaitHandleCannotBeOpenedException] { } # No remaining owner/handle.
if ($lease) {
    $acquired = $false
    try {
        try { $acquired = $lease.WaitOne(30000) }
        catch [Threading.AbandonedMutexException] { $acquired = $true }
        if (-not $acquired) { throw 'Installed startup did not release the update lease within 30 seconds' }
    } finally {
        if ($acquired) { $lease.ReleaseMutex() }
        $lease.Dispose()
    }
}
}
Wait-RecoveryStartup
& $Maintenance --prepare-install $candidateRoot
if ($LASTEXITCODE -ne 0) { throw 'Cannot quiesce recovery fixture' }
# Inject a persisted interrupted-activation state into this disposable fixture.
# This exercises the real signed recovery helper and SCM adapter. It does not
# pretend to kill the kernel, reboot Windows or reproduce physical power loss.
Write-InterruptedRecoveryFixture -Root $root -Journal $journal
foreach ($attempt in 1..2) {
    if ($attempt -eq 2) {
        # Reproduce an idle service/closed desktop deterministically before retry.
        Wait-RecoveryStartup
        & $Maintenance --prepare-install $previousRoot
        if ($LASTEXITCODE -ne 0) { throw 'Cannot quiesce restored fixture before recovery retry' }
    }
    $process = Start-Process -FilePath (Join-Path $root 'AtlasUpdater.exe') -ArgumentList '--recover' -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $root "recovery-$attempt.log") -RedirectStandardError (Join-Path $root "recovery-$attempt.err")
    $null = $process.Handle
    if (-not $process.WaitForExit(90000)) { $process.Kill(); throw 'Installed recovery timed out' }
    $process.WaitForExit()
    if ($null -eq $process.ExitCode -or $process.ExitCode -ne 0) {
        Get-Content -LiteralPath (Join-Path $root "recovery-$attempt.err")
        throw "Installed recovery failed: $($process.ExitCode)"
    }
    $restored = Get-Content -LiteralPath $journalPath -Raw | ConvertFrom-Json
    if ($restored.stage -ne 'RolledBack' -or $restored.active.id -ne $previous -or $restored.network_state -ne 'restored' -or $restored.migration_state -ne 'restored') { throw 'Recovery did not durably restore the previous version and state' }
    $service = Get-CimInstance Win32_Service -Filter "Name='AtlasNetworkService'"
    if ($service.PathName -ne ('"' + (Join-Path $previousRoot 'Atlas.Service.exe') + '" --network-service') -or $service.State -ne 'Running') { throw 'Recovery did not restore the running previous service' }
    $inspection = & $Maintenance --inspect | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or -not $inspection.baselineReady) { throw 'Recovery left an unverified network baseline' }
    if (-not (Test-Path -LiteralPath (Join-Path $previousRoot 'Atlas.exe')) -or -not (Test-Path -LiteralPath (Join-Path $candidateRoot 'Atlas.exe'))) { throw 'Recovery lost a retained version' }
}
Write-Output 'PASS: real installed recovery from persisted HealthPending restores previous service, data/network state and retained versions; repeated recovery succeeds. Physical reboot not tested.'
