param([string]$Script,[hashtable]$Parameters=@{},[switch]$Worker,[switch]$Bootstrap,[string]$Ticket)
$ErrorActionPreference='Stop'
if($Worker) {
    $identity=[Security.Principal.WindowsIdentity]::GetCurrent()
    if($identity.Name.Split('\')[-1] -notmatch '^AtlasProbe[0-9a-f]{8}$'){throw 'Worker requires the private disposable fixture identity'}
    $config=Get-Content -LiteralPath $Ticket -Raw | ConvertFrom-Json
    if($config.sid -ne $identity.User.Value){throw 'Worker ticket does not match the private user'}
    $root=Split-Path $Ticket -Parent
    $report=Join-Path $root $(if($Bootstrap){'bootstrap.log'}else{'installed-desktop.log'})
    $ownedShell=$null
    Start-Transcript -LiteralPath $report -Force | Out-Null
    try {
        $profile=(Get-ItemProperty -LiteralPath ('HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\'+$identity.User.Value)).ProfileImagePath
        $env:USERPROFILE=[Environment]::ExpandEnvironmentVariables($profile)
        $env:LOCALAPPDATA=Join-Path $env:USERPROFILE 'AppData/Local'
        $env:APPDATA=Join-Path $env:USERPROFILE 'AppData/Roaming'
        $env:TEMP=Join-Path $env:LOCALAPPDATA 'Temp';$env:TMP=$env:TEMP
        New-Item -ItemType Directory -Path $env:TEMP -Force | Out-Null
        $env:GITHUB_ACTIONS='true';$env:RUNNER_OS='Windows';$env:RUNNER_TEMP=$config.runnerTemp
        $env:PATH=$config.toolPath
        if($config.buildId){$env:ATLAS_BUILD_ID=$config.buildId}
        Set-Location -LiteralPath $config.repository
        Add-Type -Path (Join-Path $PSScriptRoot 'installed-shell.cs')
        Add-Type -Path (Join-Path $PSScriptRoot 'installed-test-process.cs')
        $principal=[Security.Principal.WindowsPrincipal]::new($identity)
        $admin=$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        if($Bootstrap) {
            if($admin -or [AtlasInstalledShell]::Integrity([uint32]$PID) -ne 8192){throw 'Interactive bootstrap must have ordinary user rights'}
            $shell=Start-Process -FilePath "$env:WINDIR/explorer.exe" -PassThru -WindowStyle Hidden
            $deadline=[DateTime]::UtcNow.AddSeconds(20)
            do {
                $shellPid=[AtlasInstalledShell]::ShellPid()
                if($shellPid){[AtlasInstalledShell]::VerifyShell($shellPid);break}
                if($shell.HasExited -and $shell.ExitCode -ne 0){throw "Ordinary Explorer exited: $($shell.ExitCode)"}
                Start-Sleep -Milliseconds 100
            } while([DateTime]::UtcNow -lt $deadline)
            if(-not $shellPid){throw 'Ordinary Explorer did not establish the notification area'}
            [AtlasInstalledShell]::VerifyShell($shellPid)
            $ownedShell=$shellPid
            if([AtlasInstalledShell]::Integrity($shellPid) -ne 8192){throw 'Fixture Explorer must remain at medium integrity'}
            Start-Sleep -Seconds 20
            Add-Type -Path (Join-Path $PSScriptRoot 'installed-tray-preflight.cs') -ReferencedAssemblies System,System.Windows.Forms,System.Drawing
            [AtlasInstalledTrayPreflight]::Verify()
            [IO.File]::WriteAllText((Join-Path $root 'ready'),[string]$shellPid)
            Write-Output "PASS: ordinary desktop and actual fixture notification registration; shell PID=$shellPid"
            while(-not (Test-Path -LiteralPath (Join-Path $root 'stop'))) {
                if(-not (Get-Process -Id $shellPid -ErrorAction SilentlyContinue) -and -not (Test-Path -LiteralPath (Join-Path $root 'stop'))){throw 'Ordinary shell terminated during installed acceptance'}
                Start-Sleep -Milliseconds 250
            }
        } else {
            if(-not $admin){throw 'Installation worker requires its own administrative batch token'}
            $shellPid=[AtlasInstalledShell]::ShellPid();[AtlasInstalledShell]::VerifyShell($shellPid)
            if([AtlasInstalledShell]::Integrity($shellPid) -ne 8192){throw 'Installation worker requires a normal medium shell of the same user'}
            $arguments=@{};foreach($property in $config.parameters.PSObject.Properties){$arguments[$property.Name]=$property.Value}
            $traceId='AtlasFixtureProcessStop'
            Register-CimIndicationEvent -Query "SELECT * FROM Win32_ProcessStopTrace WHERE ProcessName='Atlas.exe' OR ProcessName='AtlasUpdater.exe' OR ProcessName='AtlasMaintenance.exe'" -SourceIdentifier $traceId | Out-Null
            try { & $config.script @arguments }
            finally {
                Get-Event -SourceIdentifier $traceId -ErrorAction SilentlyContinue | ForEach-Object {
                    $eventData=$_.SourceEventArgs.NewEvent
                    'process='+$eventData.ProcessName+' pid='+$eventData.ProcessID+' parent='+$eventData.ParentProcessID+' exit=0x'+([uint32]$eventData.ExitStatus).ToString('X8')
                } | Set-Content -LiteralPath (Join-Path $root 'process-exits.log')
                Unregister-Event -SourceIdentifier $traceId -ErrorAction SilentlyContinue
            }
        }
    } catch {$_ | Out-String | Write-Output;throw} finally {
        if($ownedShell -and (Get-Process -Id $ownedShell -ErrorAction SilentlyContinue)){[AtlasInstalledTestProcess]::StopPrivateShell([uint32]$ownedShell,$identity.User.Value)}
        Stop-Transcript | Out-Null
    }
    return
}
if($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows'){throw 'Disposable GitHub Windows runner required'}
$repository=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$scriptPath=(Resolve-Path -LiteralPath $Script).Path
if(-not $scriptPath.StartsWith($repository+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Installed action must belong to this checkout'}
$root=Join-Path $env:RUNNER_TEMP ('atlas-installed-desktop-'+[guid]::NewGuid().ToString('N'))
$account=$null;$desktopLease=$null;$policy=$null;$bootstrap=$null;$workerProcess=$null
Add-Type -Path (Join-Path $PSScriptRoot 'installed-shell.cs')
Add-Type -Path (Join-Path $PSScriptRoot 'installed-desktop-lease.cs')
Add-Type -Path (Join-Path $PSScriptRoot 'installed-test-process.cs')
. (Join-Path $PSScriptRoot 'shell-restart-policy.ps1')
try {
    New-Item -ItemType Directory -Path $root | Out-Null
    $name='AtlasProbe'+[guid]::NewGuid().ToString('N').Substring(0,8)
    $password=[Convert]::ToBase64String([Security.Cryptography.RandomNumberGenerator]::GetBytes(48))+'!aA1'
    $credential=[PSCredential]::new($name,(ConvertTo-SecureString $password -AsPlainText -Force));$password=$null
    $account=New-LocalUser -Name $name -Password $credential.Password -PasswordNeverExpires
    Add-LocalGroupMember -SID 'S-1-5-32-544' -Member $account
    & icacls.exe $root /grant ('*'+$account.SID.Value+':(OI)(CI)F') | Out-Null
    if($LASTEXITCODE -ne 0){throw 'Private fixture evidence permissions failed'}
    $ticketPath=Join-Path $root 'action.json'
    @{sid=$account.SID.Value;repository=$repository;runnerTemp=$env:RUNNER_TEMP;toolPath=$env:PATH;buildId=$env:ATLAS_BUILD_ID;script=$scriptPath;parameters=$Parameters} |
        ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $ticketPath -Encoding utf8
    $desktopLease=[AtlasInstalledDesktopLease]::new($account.SID.Value)
    $policy=[Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon',$true)
    Invoke-ShellRestartPolicyLease -Registry $policy -Body {
        try {
            $existing=[AtlasInstalledShell]::ShellPid();[AtlasInstalledShell]::VerifyShell($existing)
            [AtlasInstalledShell]::StopVerifiedShell($existing)
            $command='-NoProfile -ExecutionPolicy Bypass -File "'+$PSCommandPath+'" -Worker -Ticket "'+$ticketPath+'"'
            $bootstrap=Start-Process -FilePath "$env:WINDIR/System32/WindowsPowerShell/v1.0/powershell.exe" -Credential $credential -LoadUserProfile -ArgumentList ($command+' -Bootstrap') -PassThru -WindowStyle Hidden
            $deadline=[DateTime]::UtcNow.AddSeconds(70)
            while(-not (Test-Path -LiteralPath (Join-Path $root 'ready')) -and -not $bootstrap.HasExited -and [DateTime]::UtcNow -lt $deadline){Start-Sleep -Milliseconds 100}
            if(Test-Path -LiteralPath (Join-Path $root 'bootstrap.log')){Get-Content -LiteralPath (Join-Path $root 'bootstrap.log') | Write-Output}
            if(-not (Test-Path -LiteralPath (Join-Path $root 'ready'))){throw 'Ordinary desktop did not pass real notification preflight'}
            $plain=[Runtime.InteropServices.Marshal]::SecureStringToGlobalAllocUnicode($credential.Password)
            try{$workerProcess=[AtlasInstalledTestProcess]::new($name,$account.SID.Value,[Runtime.InteropServices.Marshal]::PtrToStringUni($plain),(Get-Process -Id $PID).Path,$command,$repository)}
            finally{[Runtime.InteropServices.Marshal]::ZeroFreeGlobalAllocUnicode($plain)}
            if(-not $workerProcess.WaitForExit(2100000)){throw 'Installed acceptance exceeded its bounded deadline'}
            if(Test-Path -LiteralPath (Join-Path $root 'installed-desktop.log')){Get-Content -LiteralPath (Join-Path $root 'installed-desktop.log') | Write-Output}
            if($workerProcess.ExitCode -ne 0){throw "Installed acceptance worker failed: exit=$($workerProcess.ExitCode)"}
        } finally {
            if($workerProcess){$workerProcess.Dispose()}
            [IO.File]::WriteAllText((Join-Path $root 'stop'),'')
            if($bootstrap){if(-not $bootstrap.WaitForExit(10000)){$bootstrap.Kill();$bootstrap.WaitForExit()};$bootstrap.Dispose()}
            $privateShell=[AtlasInstalledShell]::ShellPid()
            if($privateShell){[AtlasInstalledTestProcess]::StopPrivateShell($privateShell,$account.SID.Value)}
        }
    }
} finally {
    if($policy){$policy.Dispose()}
    if($desktopLease){$desktopLease.Dispose()}
    if($account){Remove-LocalUser -SID $account.SID}
    $credential=$null
    if(-not [AtlasInstalledShell]::ShellPid()){Start-Process -FilePath "$env:WINDIR/explorer.exe" -WindowStyle Hidden}
}
