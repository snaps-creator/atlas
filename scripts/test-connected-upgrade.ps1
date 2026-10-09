param([Parameter(Mandatory=$true)][string]$Installer,[switch]$LegacyBaseline)
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
function Diagnose-RestrictedHelper([string]$helper) {
    # Execute the production token-launch code in a small native harness on the
    # disposable runner. The child only reads --protocol; no service/network API.
    $probe = Join-Path $fixture 'token-probe'
    New-Item -ItemType Directory -Path (Join-Path $probe 'src') -Force | Out-Null
    @'
[package]
name = "atlas-protocol-probe"
version = "2.4.2"
edition = "2021"
[dependencies]
serde_json = "1"
sha2 = "0.10"
windows-sys = {version="0.59",features=["Win32_Foundation","Win32_Security","Win32_Security_Authorization","Win32_System_Threading","Win32_System_SystemServices","Win32_System_Pipes","Win32_UI_Shell","Win32_UI_WindowsAndMessaging","Win32_System_Com"]}
'@ | Set-Content (Join-Path $probe 'Cargo.toml') -Encoding utf8
    Copy-Item (Join-Path $repo 'src-tauri/src/update_user.rs') (Join-Path $probe 'src/update_user.rs')
    @'
#[cfg(test)] mod protocol_probe {
    use super::*;
    #[test] #[ignore] fn child() {
        #[link(name="kernel32")] extern "system" {fn SetErrorMode(mode:u32)->u32;}
        unsafe{SetErrorMode(3);}
        use std::os::windows::process::CommandExt;
        assert!(desktop_safe(token().unwrap().as_raw_handle()).unwrap());
        let helper=env!("ATLAS_PROTOCOL_PROBE_HELPER");
        let mut results=Vec::new();
        results.push(serde_json::json!({"readHelperError":std::fs::File::open(helper).err().and_then(|e|e.raw_os_error()),
            "openNullError":std::fs::File::options().read(true).write(true).open("NUL").err().and_then(|e|e.raw_os_error())}));
        for flags in [0x08000000,0] {
            let result=std::process::Command::new(helper).arg("--protocol").creation_flags(flags)
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).output();
            results.push(match result {
                Ok(out)=>serde_json::json!({"flags":flags,"exit":out.status.code(),"protocolValid":serde_json::from_slice::<serde_json::Value>(&out.stdout).is_ok_and(|v|v["protocol"]==1)}),
                Err(e)=>serde_json::json!({"flags":flags,"spawnError":e.raw_os_error()})
            });
        }
        for (name,path) in [("copied-helper",env!("ATLAS_PROTOCOL_PROBE_COPY")),("system",r"C:\Windows\System32\whoami.exe")] {
            std::fs::write(env!("ATLAS_PROTOCOL_PROBE_REPORT"),serde_json::to_vec(&results).unwrap()).unwrap();
            let result=std::process::Command::new(path).arg("--protocol").creation_flags(0x08000000)
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
            results.push(match result {Ok(mut child)=>{
                let wait=unsafe{WaitForSingleObject(child.as_raw_handle(),3000)};
                if wait!=WAIT_OBJECT_0 {let _=child.kill();let _=child.wait();}
                serde_json::json!({"target":name,"started":true,"wait":wait})
            },Err(e)=>serde_json::json!({"target":name,"spawnError":e.raw_os_error()})});
        }
        std::fs::write(env!("ATLAS_PROTOCOL_PROBE_REPORT"),serde_json::to_vec(&results).unwrap()).unwrap();
    }
    #[test] fn parent() {
        let current=token().unwrap();let reduced=restricted_desktop(current.as_raw_handle()).unwrap();
        let exe=std::env::current_exe().unwrap();
        let (child,_pipe)=launch_with_token(&exe,exe.parent().unwrap(),
            &["--exact","update_user::protocol_probe::child","--ignored"],reduced.as_raw_handle()).unwrap();
        let status=unsafe{WaitForSingleObject(child.as_raw_handle(),15000)};
        if status!=WAIT_OBJECT_0 {child.terminate();panic!("Protocol probe timed out");}
        let mut exit=1;assert_ne!(unsafe{GetExitCodeProcess(child.as_raw_handle(),&mut exit)},0);assert_eq!(exit,0);
    }
}
'@ | Add-Content (Join-Path $probe 'src/update_user.rs') -Encoding utf8
    $transaction = Get-Content (Join-Path $repo 'src-tauri/src/update_transaction.rs') -Raw
    $digest = [regex]::Match($transaction, '(?ms)^pub fn digest\(.*?^\}').Value
    if (-not $digest) { throw 'Production digest function unavailable' }
    ('use std::{path::Path,fs::OpenOptions,io::Read}; use sha2::{Digest,Sha256}; type Result<T> = std::result::Result<T,String>;' + $digest) |
        Set-Content (Join-Path $probe 'src/update_transaction.rs') -Encoding utf8
    # The production token tests also validate installation ACLs. Compile the
    # exact protection routines so this diagnostic remains representative.
    $windows = Get-Content (Join-Path $repo 'src-tauri/src/update_windows.rs') -Raw
    $protection = [regex]::Match($windows, '(?ms)^pub fn protect_installation.*?^\}').Value
    if (-not $protection) { throw 'Production installation protection unavailable' }
    $protection | Set-Content (Join-Path $probe 'src/update_windows.rs') -Encoding utf8
    '#![windows_subsystem = "windows"]
mod update_user;
mod update_transaction;
mod update_windows;
fn main() {}' | Set-Content (Join-Path $probe 'src/main.rs') -Encoding utf8
    $env:ATLAS_PROTOCOL_PROBE_HELPER = $helper
    $env:ATLAS_PROTOCOL_PROBE_COPY = Join-Path $probe 'AtlasMaintenance.exe'
    Copy-Item -LiteralPath $helper -Destination $env:ATLAS_PROTOCOL_PROBE_COPY
    $env:ATLAS_PROTOCOL_PROBE_REPORT = Join-Path $fixture 'restricted-helper-codes.log'
    . (Join-Path $PSScriptRoot 'initialize-msvc.ps1')
    & cargo test --manifest-path (Join-Path $probe 'Cargo.toml') -- --exact update_user::protocol_probe::parent
    if ($LASTEXITCODE -ne 0) { Write-Output 'Restricted helper diagnostic could not finish' }
    if (Test-Path $env:ATLAS_PROTOCOL_PROBE_REPORT) { Get-Content $env:ATLAS_PROTOCOL_PROBE_REPORT }
}
function Recovery-Root {
    $registered = Get-CimInstance Win32_Service -Filter "Name='AtlasNetworkService'"
    if ($registered.PathName -eq ('"' + (Join-Path $installRoot 'Atlas.Service.exe') + '" --network-service')) { return $installRoot }
    $journalPath = Join-Path $installRoot 'current.json'
    if (Test-Path -LiteralPath $journalPath) {
        $journal = Get-Content -LiteralPath $journalPath -Raw | ConvertFrom-Json
        if ($journal.active.absent -ne $true -and $journal.active.id -match '^[a-zA-Z0-9_-][a-zA-Z0-9_.-]{0,99}$') {
            $directory = Join-Path $installRoot ('versions/' + $journal.active.id)
            if (Test-Path -LiteralPath (Join-Path $directory 'Atlas.Service.exe')) { return $directory }
        }
    }
    return $installRoot
}
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
        # Never export incident contents. Emit only source locations of known
        # literal messages found in the disposable runner's history.
        $history = Join-Path $env:LOCALAPPDATA 'net.atlasvpn.desktop/incident-history.ndjson'
        if (Test-Path -LiteralPath $history) {
            $incidentText = (Get-Content -LiteralPath $history -Tail 100 -Encoding UTF8) -join "`n"
            $categories = @('history_present')
            foreach ($kind in @('application_start','frontend_event','incident_summary')) {
                if ($incidentText.Contains('"kind":"' + $kind + '"')) { $categories += $kind }
            }
            foreach ($source in @('lib.rs','broker.rs','service.rs','update_process.rs')) {
                $lineNumber = 0
                foreach ($line in (Get-Content -LiteralPath (Join-Path $repo "src-tauri/src/$source") -Encoding UTF8)) {
                    $lineNumber++
                    foreach ($literal in [regex]::Matches($line, '"([^"\\]{16,})"')) {
                        if ($incidentText.Contains($literal.Groups[1].Value)) {
                            $categories += "${source}:$lineNumber"
                        }
                    }
                }
            }
            $categories | Sort-Object -Unique | Tee-Object -FilePath (Join-Path $fixture 'candidate-health-categories.log')
        }
        $journalPath = Join-Path $installRoot 'current.json'
        if (Test-Path -LiteralPath $journalPath) {
            $journal = Get-Content -LiteralPath $journalPath -Raw | ConvertFrom-Json
            $journal |
                Select-Object stage,sequence,network_state,health_state,migration_state |
                ConvertTo-Json | Tee-Object -FilePath (Join-Path $fixture 'transaction-state.log')
        }
        Get-CimInstance Win32_Service -Filter "Name='AtlasNetworkService'" | Select-Object Name,State,PathName | Format-List
        Get-CimInstance Win32_Process | Where-Object { $_.Name -in @('Atlas.exe','Atlas.Service.exe','Atlas.Core.exe','Atlas.Xray.exe') } |
            Select-Object ProcessId,ParentProcessId,Name,ExecutablePath | Format-List
        Get-Item (Join-Path $installRoot 'Atlas.exe') | ForEach-Object { $_.VersionInfo | Select-Object FileName,FileVersion,ProductVersion | Format-List }
        & $maintenance --inspect
        & $maintenance --prepare-install (Recovery-Root) 2>&1 | Tee-Object -FilePath (Join-Path $fixture 'recovery-after-error.log')
        Write-Output "Independent recovery exit: $LASTEXITCODE"
        if ($transactional -and $journal.candidate.id -match '^[a-zA-Z0-9_-][a-zA-Z0-9_.-]{0,99}$') {
            Diagnose-RestrictedHelper (Join-Path $installRoot ('versions/' + $journal.candidate.id + '/AtlasMaintenance.exe'))
        }
        throw "Installer failed: $($process.ExitCode)"
    }
}
$previousVersion = if ($LegacyBaseline) { '2.2.3-alpha.47.1' } else { '2.4.2' }
$previousName = "Atlas_${previousVersion}_x64-setup.exe"
$previous = Join-Path $fixture $previousName
$base = "https://github.com/snaps-creator/atlas/releases/download/v$previousVersion/"
Invoke-WebRequest ($base + $previousName) -OutFile $previous
Invoke-WebRequest ($base + $previousName + '.sig') -OutFile "$previous.sig"
node (Join-Path $PSScriptRoot 'verify-installer.cjs') $previous
if ($LASTEXITCODE -ne 0) { throw 'Old release signature does not match Atlas trust' }
$previousRoot = $installRoot
$core = $null
$desktop = $null
$fixtureServer = $null
try {
if ($LegacyBaseline) {
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
} else {
    # Use a real authenticated VLESS peer on the disposable runner. A closed
    # synthetic port correctly fails Atlas's pinned-path validation even in
    # DIRECT routing mode. No production validation is disabled for acceptance.
    $network = Get-NetIPConfiguration | Where-Object IPv4DefaultGateway | Select-Object -First 1
    $peerAddress = @($network.IPv4Address)[0].IPAddress
    if (-not $peerAddress) { throw 'No IPv4 interface for the disposable VLESS peer' }
    $reservation = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Parse($peerAddress),0)
    $reservation.Start()
    $peerPort = $reservation.LocalEndpoint.Port
    $reservation.Stop()
    & python (Join-Path $PSScriptRoot 'installed-settings-fixture.py') seed $peerPort $peerAddress
    if ($LASTEXITCODE -ne 0) { throw 'Mixed-source fixture initialization failed' }
    $peerExecutable = Join-Path $fixture 'fixture-vless-server.exe'
    Copy-Item (Join-Path $candidateFiles '$PLUGINSDIR/payload/resources/Atlas.Xray.exe') $peerExecutable
    $peerConfig = Join-Path $env:RUNNER_TEMP 'atlas-acceptance-vless.json'
    $fixtureServer = Start-Process -FilePath $peerExecutable -ArgumentList @('run','-config',"`"$peerConfig`"") -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $fixture 'peer.log') -RedirectStandardError (Join-Path $fixture 'peer.err')
    $peerReady = $false
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        if ($fixtureServer.HasExited) { throw 'Disposable VLESS peer failed to start' }
        $socket = [Net.Sockets.TcpClient]::new()
        try { $peerReady = $socket.ConnectAsync($peerAddress,$peerPort).Wait(500) -and $socket.Connected } catch {} finally { $socket.Dispose() }
        if (-not $peerReady) { Start-Sleep -Milliseconds 100 }
    } while (-not $peerReady -and [DateTime]::UtcNow -lt $deadline)
    if (-not $peerReady) { throw 'Disposable VLESS peer did not open its listener' }
    Install-Checked $previous
    $oldJournal=Get-Content (Join-Path $installRoot 'current.json') -Raw | ConvertFrom-Json
    if ($oldJournal.stage -ne 'Committed' -or $oldJournal.active.version -ne '2.4.2') { throw 'Real 2.4.2 installation was not committed' }
    $previousRoot=Join-Path $installRoot ('versions/' + $oldJournal.active.id)
    & $maintenance --prepare-install $previousRoot
    if ($LASTEXITCODE -ne 0) { throw 'Previous installed application did not quiesce' }
    & python (Join-Path $PSScriptRoot 'installed-settings-fixture.py') prepare-upgrade
    if ($LASTEXITCODE -ne 0) { throw 'Valid installed 2.4.2 references were not established' }
}
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
    $core = Start-Process -FilePath (Join-Path $previousRoot 'resources/Atlas.Core.exe') -ArgumentList @('-d',"`"$fixture`"",'-f',"`"$config`"") -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $fixture 'core.log') -RedirectStandardError (Join-Path $fixture 'core.err')
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        if ($core.HasExited) { throw "Old core exited: $(Get-Content (Join-Path $fixture 'core.log') -Raw)" }
        $adapter = Get-NetAdapter -Name Atlas-TUN -ErrorAction SilentlyContinue
        if ($adapter.Status -eq 'Up') { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($adapter.Status -ne 'Up') { throw 'Fixture did not establish a real active Atlas-TUN' }
    # Keep an actual old Atlas desktop host open without user settings/UI/VPN.
    $start = [Diagnostics.ProcessStartInfo]::new((Join-Path $previousRoot 'Atlas.exe'), '--identity-owner')
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
        $expectedVersion = if ($LegacyBaseline) { '2.4.2' } else { (Get-Content (Join-Path $repo 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version }
        if ($journal.active.version -ne $expectedVersion) { throw 'Wrong active version after upgrade' }
        # Commit already checked authenticated UI/service readiness. Stop this
        # disposable runner's candidate before the independent TUN reuse test.
        & $maintenance --prepare-install (Recovery-Root)
        if ($LASTEXITCODE -ne 0) { throw 'Committed candidate did not quiesce' }
    }
    $inspection = & (Join-Path $activeRoot 'AtlasMaintenance.exe') --inspect | ConvertFrom-Json
    $baseline = if ($inspection.PSObject.Properties.Name -contains 'baselineReady') { $inspection.baselineReady } else { $null -eq $inspection.activeLuid }
    if ($LASTEXITCODE -ne 0 -or -not $baseline) { throw 'Update left an active or unverified Atlas-TUN' }
    if ((Get-Service AtlasNetworkService).Status -ne 'Stopped') { throw 'Service still runs after update' }
    Write-Output "PASS: signed $previousVersion -> candidate upgrade with real active Wintun, service and desktop."
    # Retain the original first-post-upgrade smoke position. A failure remains
    # fatal at the end, but must not hide independent installed acceptance.
    $isolatedUiError = $null
    try { & (Join-Path $PSScriptRoot 'test-ui-startup.ps1') -Executable (Join-Path $activeRoot 'Atlas.exe') }
    catch { $isolatedUiError = $_.Exception.Message; Write-Warning "Isolated UI failure retained: $isolatedUiError" }
    if (-not $LegacyBaseline) {
        & python (Join-Path $PSScriptRoot 'installed-settings-fixture.py') verify
        if ($LASTEXITCODE -ne 0) { throw 'Upgrade lost mixed sources or persisted references/credentials' }
        & (Join-Path $PSScriptRoot 'test-installed-startup.ps1') -InstallRoot $installRoot -Maintenance $maintenance
    }
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
        & (Join-Path $PSScriptRoot 'test-installed-recovery.ps1') -InstallRoot $installRoot -Maintenance $maintenance
        Install-Checked $candidate
        $reinstalled = Get-Content -LiteralPath (Join-Path $installRoot 'current.json') -Raw | ConvertFrom-Json
        if ($reinstalled.stage -ne 'Committed' -or $reinstalled.active.id -ne $journal.candidate.id) { throw 'Reinstall after recovery did not commit the candidate' }
        Write-Output 'PASS: candidate reinstalls successfully after installed rollback and repeated recovery.'
        foreach ($cycle in @('upgrade uninstall','clean install uninstall')) {
            $runKey='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
            $approvedKey='HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'
            if (-not $LegacyBaseline) {
                New-Item -Path $runKey,$approvedKey -Force | Out-Null
                New-ItemProperty -Path $runKey -Name Atlas -PropertyType String -Value ('"'+(Join-Path $installRoot 'AtlasUpdater.exe')+'" --launch --autostart') -Force | Out-Null
                New-ItemProperty -Path $approvedKey -Name Atlas -PropertyType Binary -Value ([byte[]](2,0,0,0,0,0,0,0,0,0,0,0)) -Force | Out-Null
            }
            $uninstaller = Join-Path $installRoot 'uninstall.exe'
            $remove = Start-Process -FilePath $uninstaller -ArgumentList @('/S',"_?=$installRoot") -WindowStyle Hidden -PassThru
            $null = $remove.Handle
            if (-not $remove.WaitForExit(180000)) { throw "Timed out: $cycle" }
            $remove.WaitForExit()
            if ($null -eq $remove.ExitCode -or $remove.ExitCode -ne 0) {
                $log = Join-Path $env:TEMP 'atlas-transactional-uninstall.log'
                if (Test-Path -LiteralPath $log) {
                    Copy-Item -LiteralPath $log -Destination (Join-Path $fixture 'atlas-transactional-uninstall.log')
                    Get-Content -LiteralPath $log -Tail 40
                }
                throw "Failed ${cycle}: exit=$($remove.ExitCode)"
            }
            if (Get-Service AtlasNetworkService -ErrorAction SilentlyContinue) { throw 'Uninstall left the Atlas service' }
            foreach ($remaining in @('Atlas.exe','AtlasUpdater.exe','current.json','versions')) {
                if (Test-Path -LiteralPath (Join-Path $installRoot $remaining)) { throw "Uninstall left $remaining" }
            }
            if (Test-Path -LiteralPath 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Atlas') {
                throw 'Uninstall left its registration'
            }
            $inspection = & $maintenance --inspect | ConvertFrom-Json
            if ($LASTEXITCODE -ne 0 -or -not $inspection.baselineReady) { throw 'Uninstall did not restore the network baseline' }
            if (-not $LegacyBaseline) {
                foreach ($key in @($runKey,$approvedKey)) {
                    if ((Get-Item -LiteralPath $key).GetValueNames() -contains 'Atlas') { throw 'Uninstall left Atlas startup registration' }
                }
            }
            & curl.exe --fail --silent --max-time 20 https://example.com/ --output NUL
            if ($LASTEXITCODE -ne 0) { throw 'Direct network control failed after uninstall' }
            Write-Output "PASS: $cycle; service removed, startup cleaned and network baseline restored."
            if ($cycle -eq 'upgrade uninstall') { Install-Checked $candidate }
        }
    }
    if ($isolatedUiError) { throw "Installed checks completed, but isolated UI lifecycle failed: $isolatedUiError" }
} catch {
    # Keep the primary failure visible even if the independent cleanup fails.
    Write-Output "Installed acceptance failure: $($_.Exception.Message)"
    throw
} finally {
    foreach ($process in @($core,$desktop,$fixtureServer)) {
        if ($null -ne $process -and -not $process.HasExited) { $process.Kill(); $process.WaitForExit(5000) | Out-Null }
    }
    # The guard above established that this disposable job created the service.
    if (Get-Service AtlasNetworkService -ErrorAction SilentlyContinue) {
        & $maintenance --prepare-install (Recovery-Root)
        if ($LASTEXITCODE -ne 0) { throw 'Disposable fixture cleanup failed' }
        sc.exe delete AtlasNetworkService | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'Disposable fixture service removal failed' }
    }
}
