param([Parameter(Mandatory=$true)][string]$InstallRoot,[Parameter(Mandatory=$true)][string]$Maintenance)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Disposable GitHub Windows runner required' }
$journal = Get-Content (Join-Path $InstallRoot 'current.json') -Raw | ConvertFrom-Json
if ($journal.stage -ne 'Committed') { throw 'Candidate must be committed before startup acceptance' }
$active = Join-Path $InstallRoot ('versions/' + $journal.active.id)
$desktopExe = Join-Path $active 'Atlas.exe'
$settingsFixture = Join-Path $PSScriptRoot 'installed-settings-fixture.py'
$history = Join-Path $env:LOCALAPPDATA 'net.atlasvpn.desktop/incident-history.ndjson'
Add-Type -TypeDefinition @'
using System;
using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
public static class AtlasWindowHistory {
    public static readonly ConcurrentQueue<uint> VisiblePids = new ConcurrentQueue<uint>();
    delegate void Callback(IntPtr hook,uint ev,IntPtr hwnd,int obj,int child,uint thread,uint time);
    static Callback callback;
    static IntPtr hook;
    static Thread worker;
    static uint threadId;
    [StructLayout(LayoutKind.Sequential)] struct Message { public IntPtr hwnd; public uint msg; public UIntPtr w; public IntPtr l; public uint time; public int x,y; public uint priv; }
    [DllImport("user32.dll")] static extern IntPtr SetWinEventHook(uint min,uint max,IntPtr module,Callback cb,uint pid,uint tid,uint flags);
    [DllImport("user32.dll")] static extern bool UnhookWinEvent(IntPtr h);
    [DllImport("user32.dll")] static extern int GetMessage(out Message m,IntPtr h,uint min,uint max);
    [DllImport("user32.dll")] static extern bool PostThreadMessage(uint tid,uint msg,UIntPtr w,IntPtr l);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h,StringBuilder text,int max);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int cmd);
    public static void Start() {
        uint ignored; while(VisiblePids.TryDequeue(out ignored)) {}
        var ready=new ManualResetEventSlim();
        worker=new Thread(()=> {
            threadId=GetCurrentThreadId();
            callback=(h,e,window,obj,child,t,ms)=> {
                if(obj!=0 || child!=0) return;
                var title=new StringBuilder(512); GetWindowText(window,title,512);
                if(title.ToString().StartsWith("Atlas") && IsWindowVisible(window)) {
                    uint pid; GetWindowThreadProcessId(window,out pid); VisiblePids.Enqueue(pid);
                }
            };
            hook=SetWinEventHook(0x8002,0x8002,IntPtr.Zero,callback,0,0,0);
            ready.Set(); Message message;
            while(GetMessage(out message,IntPtr.Zero,0,0)>0) {}
            UnhookWinEvent(hook);
        });
        worker.IsBackground=true; worker.Start();
        if(!ready.Wait(5000) || hook==IntPtr.Zero) throw new Exception("Window event observer unavailable");
    }
    public static void Stop() { PostThreadMessage(threadId,0x12,UIntPtr.Zero,IntPtr.Zero); if(!worker.Join(5000)) throw new Exception("Window observer failed to stop"); }
}
'@
function Quiesce {
    & $Maintenance --prepare-install $active
    if ($LASTEXITCODE -ne 0) { throw 'Candidate did not quiesce safely' }
}
function Fixture([string]$Action, [string]$Value) {
    $result = & python $settingsFixture $Action $Value
    if ($LASTEXITCODE -ne 0) { throw "Installed state fixture failed: $Action" }
    return $result
}
function Events {
    if (Test-Path -LiteralPath $history) {
        Get-Content -LiteralPath $history | ForEach-Object {
            try { $entry=$_ | ConvertFrom-Json; [pscustomobject]@{kind=$entry.kind;value=($entry.evidence | ConvertFrom-Json)} } catch {}
        }
    }
}
$cases = @(
    @{name='A';windows=$true;auto=$false;tray=$false;restore=$false;was=$false;explicit=$false;connect=$false},
    @{name='B';windows=$true;auto=$false;tray=$true;restore=$false;was=$false;explicit=$false;connect=$false},
    @{name='C';windows=$true;auto=$true;tray=$true;restore=$true;was=$true;explicit=$false;connect=$true},
    @{name='D';windows=$false;auto=$true;tray=$false;restore=$false;was=$false;explicit=$false;connect=$true},
    @{name='E';windows=$false;auto=$true;tray=$true;restore=$false;was=$false;explicit=$false;connect=$true},
    @{name='F';windows=$false;auto=$false;tray=$true;restore=$true;was=$true;explicit=$false;connect=$true},
    @{name='G';windows=$false;auto=$false;tray=$false;restore=$true;was=$false;explicit=$true;connect=$false},
    @{name='H';windows=$false;auto=$false;tray=$true;restore=$false;was=$false;explicit=$true;connect=$false}
)
try {
    Quiesce
    Fixture verify | Write-Output
    foreach ($case in $cases) {
        $patch=@{startup=@{launchWithWindows=$case.windows;autoConnect=$case.auto;startInTray=$case.tray;restoreConnection=$case.restore;delaySeconds=1};wasConnected=$case.was;userDisconnected=$case.explicit;lastWindowHidden=$case.tray}
        if ($case.name -eq 'H') { $patch.startup.startInTray=$false } # Manual restart retains recorded hidden state.
        Fixture patch ($patch | ConvertTo-Json -Compress) | Out-Null
        if (Test-Path -LiteralPath $history) { Clear-Content -LiteralPath $history }
        [AtlasWindowHistory]::Start()
        try {
            $arguments=@('--launch'); if ($case.windows) { $arguments+='--autostart' }
            if ($case.name -eq 'H') {
                $launch=Start-Process -FilePath $desktopExe -ArgumentList '--manual-restart' -PassThru -WindowStyle Hidden
            } else {
                $launch=Start-Process -FilePath (Join-Path $InstallRoot 'AtlasUpdater.exe') -ArgumentList $arguments -PassThru -WindowStyle Hidden
                if (-not $launch.WaitForExit(30000) -or $launch.ExitCode -ne 0) { throw 'Stable launcher failed' }
            }
            $deadline=[DateTime]::UtcNow.AddSeconds(55)
            do {
                $owners=@(Get-CimInstance Win32_Process -Filter "Name='Atlas.exe'" | Where-Object { $_.ExecutablePath -eq $desktopExe -and $_.CommandLine -notmatch '--type=' })
                $events=@(Events)
                $samples=@($events | Where-Object kind -eq 'passive_sample')
                $expected=if($case.connect){'Connected'}else{'Disconnected'}
                if ($owners.Count -eq 1 -and $samples.Count -gt 0 -and $samples[-1].value.status -eq $expected) { break }
                Start-Sleep -Milliseconds 250
            } while ([DateTime]::UtcNow -lt $deadline)
            if ($owners.Count -ne 1 -or $samples.Count -eq 0 -or $samples[-1].value.status -ne $expected) { throw "Startup $($case.name) did not reach $expected" }
            # Observe another complete recorder interval to catch repeated connect/recovery.
            Start-Sleep -Seconds 16
            $events=@(Events)
            $connected=@($events | Where-Object { $_.kind -eq 'application_event' -and $_.value.message -eq 'Подключён режим всей системы (TUN)' })
            if ($connected.Count -ne [int]$case.connect) { throw "Startup $($case.name): expected one/zero connect, got $($connected.Count)" }
            if (@($events | Where-Object { $_.kind -eq 'passive_sample' -and $_.value.status -in @('Error','CleanupError','ProtectedPause') }).Count) { throw 'Startup entered a failed/recovery state' }
            $shown=@([AtlasWindowHistory]::VisiblePids.ToArray() | Where-Object { $_ -eq $owners[0].ProcessId })
            if ($case.tray -and $shown.Count -ne 0) { throw "Startup $($case.name): main window flashed" }
            if (-not $case.tray -and $shown.Count -eq 0) { throw "Startup $($case.name): window never became visible" }
            $saved=Fixture state | ConvertFrom-Json
            if ($saved.wasConnected -ne $case.connect -or $saved.lastWindowHidden -ne $case.tray) { throw 'Runtime state was not persisted correctly' }
            $run=Get-ItemPropertyValue 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name Atlas -ErrorAction SilentlyContinue
            if ($case.windows -and $run -ne ('"'+(Join-Path $InstallRoot 'AtlasUpdater.exe')+'" --launch --autostart')) { throw 'Startup registration does not use the current stable launcher' }
            if (-not $case.windows -and $run) { throw 'Disabled startup still registered' }
            if ($case.connect) {
                & curl.exe --fail --silent --max-time 20 --proxy http://127.0.0.1:17890 https://example.com/ --output NUL
                if ($LASTEXITCODE -ne 0) { throw 'Connected fixture cannot reach control website through core' }
            }
            Fixture verify | Write-Output
            Write-Output "PASS installed startup $($case.name): $expected; tray=$($case.tray); connect count=$($connected.Count); visible events=$($shown.Count)"
            if ($case.name -eq 'D') {
                foreach ($step in @(@{button='Отключить VPN';connected=$false},@{button='Подключить VPN';connected=$true},@{button='Отключить VPN';connected=$false})) {
                    # UIAutomation uses the real React action and backend IPC.
                    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'invoke-installed-button.ps1') -ProcessId $owners[0].ProcessId -Name $step.button
                    if ($LASTEXITCODE -ne 0) { throw 'Installed connect/disconnect interaction failed' }
                    $deadline=[DateTime]::UtcNow.AddSeconds(30)
                    do {
                        $saved=Fixture state | ConvertFrom-Json
                        if ($saved.wasConnected -eq $step.connected -and $saved.userDisconnected -eq (-not $step.connected)) { break }
                        Start-Sleep -Milliseconds 250
                    } while ([DateTime]::UtcNow -lt $deadline)
                    if ($saved.wasConnected -ne $step.connected -or $saved.userDisconnected -ne (-not $step.connected)) { throw 'Explicit connection intent was not persisted' }
                }
                Write-Output 'PASS: real UI disconnect, reconnect, explicit disconnect persisted'
            }
        } catch {
            # Only synthetic runner data is used. Restrict failure output to the
            # lifecycle fields; never dump credentials, configs or traffic logs.
            @(Events | Where-Object { $_.kind -in @('application_event','passive_sample') } | Select-Object -Last 20 |
                ForEach-Object { [pscustomobject]@{kind=$_.kind;status=$_.value.status;message=$_.value.message;error=$_.value.error} }) |
                ConvertTo-Json -Depth 3 | Write-Output
            throw
        } finally { [AtlasWindowHistory]::Stop(); Quiesce }
    }
} finally { Quiesce }
