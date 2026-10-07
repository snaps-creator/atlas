param([Parameter(Mandatory=$true)][string]$Installer)
$ErrorActionPreference = 'Stop'
# Never run this acceptance on a user's workstation: it creates a real Wintun
# adapter and installs a service. GitHub's disposable Windows job is required.
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'This test requires a disposable GitHub Windows runner' }
if (Get-Service AtlasNetworkService -ErrorAction SilentlyContinue) { throw 'Runner already contains Atlas; refusing to change it' }
if (Get-NetAdapter -Name Atlas-TUN -ErrorAction SilentlyContinue) { throw 'Runner already contains Atlas-TUN; refusing to change it' }
$repo = Split-Path $PSScriptRoot -Parent
$candidate = (Resolve-Path -LiteralPath $Installer).Path
$fixture = Join-Path $env:RUNNER_TEMP ('atlas-connected-upgrade-' + [guid]::NewGuid().ToString('N'))
$installRoot = Join-Path $fixture 'installed'
New-Item -ItemType Directory -Path $fixture | Out-Null
# WFP is a normal Windows prerequisite. Hosted CI images may stop BFE;
# explicitly prepare it only on this guarded, disposable test machine.
Get-Service BFE | Select-Object Name,Status,StartType | Format-Table
if ((Get-Service BFE).Status -ne 'Running') { Set-Service BFE -StartupType Manual; Start-Service BFE }
$sevenZip = (Get-Command 7z.exe -ErrorAction Stop).Source
$candidateFiles = Join-Path $fixture 'candidate'
& $sevenZip x $candidate "-o$candidateFiles" -y | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Candidate extraction failed' }
$maintenance = Join-Path $candidateFiles 'AtlasMaintenance.exe'
$transactional = Test-Path -LiteralPath (Join-Path $candidateFiles '$PLUGINSDIR/payload/AtlasUpdater.exe')
if ($transactional) { $maintenance = Join-Path $candidateFiles '$PLUGINSDIR/payload/AtlasMaintenance.exe' }
$activeRoot = $installRoot
function Install-Checked([string]$path) {
    $process = Start-Process -FilePath $path -ArgumentList @('/S','/UPDATE',"/D=$installRoot") -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit(180000)) { $process.Kill(); throw 'Installer exceeded three minutes' }
    if ($process.ExitCode -ne 0) {
        Write-Output "Failed installer: $path; expected installation: $installRoot"
        foreach ($logName in @('atlas-transactional-install.log','atlas-install-recovery.log')) {
            $nativeLog = Join-Path $env:TEMP $logName
            if (Test-Path -LiteralPath $nativeLog) {
                Copy-Item -LiteralPath $nativeLog -Destination (Join-Path $fixture $logName)
                Get-Content -LiteralPath $nativeLog -Tail 60
            }
        }
        Get-CimInstance Win32_Service -Filter "Name='AtlasNetworkService'" | Select-Object Name,State,PathName | Format-List
        Get-CimInstance Win32_Process | Where-Object { $_.Name -in @('Atlas.exe','Atlas.Service.exe','Atlas.Core.exe','Atlas.Xray.exe') } |
            Select-Object ProcessId,ParentProcessId,Name,ExecutablePath | Format-List
        Get-Item (Join-Path $installRoot 'Atlas.exe') | ForEach-Object { $_.VersionInfo | Select-Object FileName,FileVersion,ProductVersion | Format-List }
        & $maintenance --inspect
        & $maintenance --prepare-install $installRoot 2>&1 | Tee-Object -FilePath (Join-Path $fixture 'recovery-after-error.log')
        Write-Output "Independent recovery exit: $LASTEXITCODE"
        throw "Installer failed: $($process.ExitCode)"
    }
}
$previousName = 'Atlas_2.2.3-alpha.47.1_x64-setup.exe'
$previous = Join-Path $fixture $previousName
$base = 'https://github.com/snaps-creator/atlas/releases/download/v2.2.3-alpha.47.1/'
Invoke-WebRequest ($base + $previousName) -OutFile $previous
Invoke-WebRequest ($base + $previousName + '.sig') -OutFile "$previous.sig"
node (Join-Path $PSScriptRoot 'verify-installer.cjs') $previous
if ($LASTEXITCODE -ne 0) { throw 'Old release signature does not match Atlas trust' }
# Seed the actual signed old binaries and register their real service. The
# regression concerns upgrading an EXISTING old installation, not whether its
# obsolete cleanup hook can provision a fresh Windows Server CI image.
# NSIS expands $PLUGINSDIR into its temporary directory, never $INSTDIR.
# A raw archive extraction otherwise invents an installed "$PLUGINSDIR" folder;
# the transactional legacy inventory correctly rejects that invalid path.
& $sevenZip x $previous "-o$installRoot" '-xr!$PLUGINSDIR' -y | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Old release extraction failed' }
if (Test-Path -LiteralPath (Join-Path $installRoot '$PLUGINSDIR')) { throw 'NSIS temporary files leaked into the installed fixture' }
Copy-Item (Join-Path $installRoot 'Atlas.exe') (Join-Path $installRoot 'Atlas.Service.exe')
$register = Start-Process -FilePath (Join-Path $installRoot 'Atlas.exe') -ArgumentList '--install-service' -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $fixture 'register.log') -RedirectStandardError (Join-Path $fixture 'register.err')
if (-not $register.WaitForExit(30000) -or $register.ExitCode -ne 0) { throw "Old service registration failed: $(Get-Content (Join-Path $fixture 'register.err') -Raw)" }
$core = $null
$desktop = $null
try {
    # A real tunnel, but deliberately NO automatic/default routes, no system DNS
    # and no external VPN server. This cannot route the CI control connection.
    $config = Join-Path $fixture 'tun.yaml'
    @'
mixed-port: 0
mode: direct
log-level: info
ipv6: false
dns:
  enable: false
tun:
  enable: true
  device: Atlas-TUN
  stack: system
  auto-route: false
  strict-route: false
  auto-detect-interface: true
proxies: []
rules:
  - MATCH,DIRECT
'@ | Set-Content -LiteralPath $config -Encoding utf8
    Start-Service AtlasNetworkService
    $core = Start-Process -FilePath (Join-Path $installRoot 'resources/Atlas.Core.exe') -ArgumentList @('-d',"`"$fixture`"",'-f',"`"$config`"") -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $fixture 'core.log') -RedirectStandardError (Join-Path $fixture 'core.err')
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        if ($core.HasExited) { throw "Old core exited: $(Get-Content (Join-Path $fixture 'core.log') -Raw)" }
        $adapter = Get-NetAdapter -Name Atlas-TUN -ErrorAction SilentlyContinue
        if ($adapter.Status -eq 'Up') { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($adapter.Status -ne 'Up') { throw 'Fixture did not establish a real active Atlas-TUN' }
    # Keep an actual old Atlas desktop host open without user settings/UI/VPN.
    $start = [Diagnostics.ProcessStartInfo]::new((Join-Path $installRoot 'Atlas.exe'), '--identity-owner')
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.Environment['ATLAS_SERVICE_IDENTITY_SMOKE'] = '1'
    $desktop = [Diagnostics.Process]::Start($start)
    Start-Sleep -Milliseconds 300
    if ($desktop.HasExited) { throw 'Old desktop fixture failed to remain open' }
    Install-Checked $candidate
    if (-not $core.WaitForExit(5000) -or -not $desktop.WaitForExit(5000)) { throw 'Update left old Atlas processes running' }
    if ($transactional) {
        $journal = Get-Content -LiteralPath (Join-Path $installRoot 'current.json') -Raw | ConvertFrom-Json
        if ($journal.stage -ne 'Committed') { throw 'Upgrade was not durably committed' }
        $activeRoot = Join-Path $installRoot ('versions/' + $journal.active.id)
        $expectedVersion = (Get-Content (Join-Path $repo 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version
        if ($journal.active.version -ne $expectedVersion) { throw 'Wrong active version after upgrade' }
        # Commit already checked authenticated UI/service readiness. Stop this
        # disposable runner's candidate before the independent TUN reuse test.
        & $maintenance --prepare-install $installRoot
        if ($LASTEXITCODE -ne 0) { throw 'Committed candidate did not quiesce' }
    }
    $inspection = & (Join-Path $activeRoot 'AtlasMaintenance.exe') --inspect | ConvertFrom-Json
    $baseline = if ($inspection.PSObject.Properties.Name -contains 'baselineReady') { $inspection.baselineReady } else { $null -eq $inspection.activeLuid }
    if ($LASTEXITCODE -ne 0 -or -not $baseline) { throw 'Update left an active or unverified Atlas-TUN' }
    if ((Get-Service AtlasNetworkService).Status -ne 'Stopped') { throw 'Service still runs after update' }
    & (Join-Path $PSScriptRoot 'test-ui-startup.ps1') -Executable (Join-Path $activeRoot 'Atlas.exe')
    Write-Output 'PASS: signed 2.2.3 -> candidate upgrade with real active Wintun, service and desktop; old processes exited, tunnel released, new UI rendered.'
    $core = Start-Process -FilePath (Join-Path $activeRoot 'resources/Atlas.Core.exe') -ArgumentList @('-d',"`"$fixture`"",'-f',"`"$config`"") -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $fixture 'reconnect.log') -RedirectStandardError (Join-Path $fixture 'reconnect.err')
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        if ($core.HasExited) { throw 'New core cannot reopen the released tunnel' }
        $adapter = Get-NetAdapter -Name Atlas-TUN -ErrorAction SilentlyContinue
        if ($adapter.Status -eq 'Up') { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($adapter.Status -ne 'Up') { throw 'New core failed to reconnect after the upgrade' }
    $inspection = & (Join-Path $activeRoot 'AtlasMaintenance.exe') --inspect | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or -not $inspection.verifiedWintunDriver) { throw 'Live Wintun driver identity was not verified' }
    $core.Kill()
    $core.WaitForExit(5000) | Out-Null
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $inspection = & (Join-Path $activeRoot 'AtlasMaintenance.exe') --inspect | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0) { throw 'Post-exit adapter inspection failed' }
        $hasRecoveryBaseline = $inspection.PSObject.Properties.Name -contains 'baselineReady'
        if (($hasRecoveryBaseline -and $inspection.baselineReady -eq $true) -or
            (-not $hasRecoveryBaseline -and $null -eq $inspection.activeLuid)) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    if (($hasRecoveryBaseline -and $inspection.baselineReady -ne $true) -or
        (-not $hasRecoveryBaseline -and $null -ne $inspection.activeLuid)) { throw 'Exited core left an unverified recovery baseline' }
    if ($hasRecoveryBaseline) {
        $handshake = Start-Process -FilePath (Join-Path $activeRoot 'Atlas.exe') -ArgumentList '--check-network-service' -WindowStyle Hidden -PassThru -RedirectStandardError (Join-Path $fixture 'ipc-reconnect.err')
        if (-not $handshake.WaitForExit(25000)) { $handshake.Kill(); throw 'Service channel reconnect timed out' }
        if ($handshake.ExitCode -ne 0) { throw 'Service channel reconnect changed process/session identity or failed' }
        Write-Output 'PASS: control channel reconnect preserves SCM PID and network epoch.'
    }
    Write-Output 'PASS: new core can reopen Atlas-TUN after the upgrade.'
    $start.FileName = Join-Path $activeRoot 'Atlas.exe'
    $desktop = [Diagnostics.Process]::Start($start)
    Start-Sleep -Milliseconds 300
    if ($desktop.HasExited) { throw 'Disconnected desktop fixture failed to remain open' }
    Install-Checked $candidate
    if (-not $transactional -and -not $desktop.WaitForExit(5000)) { throw 'Update left the disconnected desktop running' }
    if (-not $desktop.HasExited) { $desktop.Kill(); $desktop.WaitForExit(5000) | Out-Null }
    Write-Output 'PASS: installation succeeds with Atlas open and tunnel disconnected.'
    Install-Checked $candidate
    Write-Output 'PASS: installation also succeeds with Atlas closed and tunnel disconnected.'
    if ($transactional) {
        foreach ($cycle in @('upgrade uninstall','clean install uninstall')) {
            $uninstaller = Join-Path $installRoot 'uninstall.exe'
            $remove = Start-Process -FilePath $uninstaller -ArgumentList @('/S',"_?=$installRoot") -WindowStyle Hidden -PassThru
            if (-not $remove.WaitForExit(180000) -or $remove.ExitCode -ne 0) { throw "Failed $cycle" }
            if (Get-Service AtlasNetworkService -ErrorAction SilentlyContinue) { throw 'Uninstall left the Atlas service' }
            $inspection = & $maintenance --inspect | ConvertFrom-Json
            if ($LASTEXITCODE -ne 0 -or -not $inspection.baselineReady) { throw 'Uninstall did not restore the network baseline' }
            Write-Output "PASS: $cycle; service removed and network baseline restored."
            if ($cycle -eq 'upgrade uninstall') { Install-Checked $candidate }
        }
    }
} finally {
    foreach ($process in @($core,$desktop)) {
        if ($null -ne $process -and -not $process.HasExited) { $process.Kill(); $process.WaitForExit(5000) | Out-Null }
    }
    # The guard above established that this disposable job created the service.
    & $maintenance --prepare-install $installRoot
    sc.exe delete AtlasNetworkService | Out-Null
}
