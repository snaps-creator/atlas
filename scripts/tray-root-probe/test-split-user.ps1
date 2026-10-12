param([switch]$Worker,[string]$Repository,[string]$Executable,[string]$Report)
$ErrorActionPreference='Stop'
if ($Worker) {
    $env:GITHUB_ACTIONS='true';$env:RUNNER_OS='Windows'
    Set-Location -LiteralPath $Repository
    Start-Transcript -LiteralPath $Report -Force | Out-Null
    try {
        Add-Type -Path scripts/installed-shell.cs
        $principal=[Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
        $admin=$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        Write-Output ('NEW USER: admin='+$admin+'; integrity='+[AtlasInstalledShell]::Integrity([uint32]$PID))
        if ($admin) { & $Executable --shell-linked-bootstrap; if($LASTEXITCODE -ne 0){throw 'Linked shell launch failed'} }
        else { Start-Process -FilePath "$env:WINDIR/explorer.exe" -WindowStyle Hidden | Out-Null }
        Start-Sleep -Seconds 20
        $shell=[AtlasInstalledShell]::ShellPid()
        if(-not $shell){throw 'Normal user shell did not stay alive'}
        [AtlasInstalledShell]::VerifyShell($shell)
        if([AtlasInstalledShell]::Integrity($shell) -ne 8192){throw 'New user shell is not medium'}
        Write-Output ('STANDARD MEDIUM SHELL STABLE: PID='+$shell)
        & $Executable
        if($LASTEXITCODE -ne 0){throw 'Normal user registration probe failed'}
        $probeRoot=Join-Path $env:LOCALAPPDATA 'AtlasTrayRootProbe-*'
        $reports=@(Get-ChildItem -Path "$probeRoot/*.json" | Where-Object Name -in 'medium.json','parent.json')
        if(-not $reports.Count){throw 'No normal user registration report'}
        foreach($file in $reports){$proof=Get-Content -LiteralPath $file.FullName -Raw|ConvertFrom-Json;if($proof.notifyAdd -ne 1){throw 'Native registration still failed for normal user'}}
        Write-Output 'PASS: actual native icon registration with a normal medium shell and unchanged application token logic'
    } finally {Stop-Transcript | Out-Null}
    return
}
if($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows'){throw 'Disposable GitHub Windows runner required'}
$repo=Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$exe=Join-Path $env:CARGO_TARGET_DIR 'debug/atlas-tray-root-probe.exe'
$name='AtlasProbe'+[guid]::NewGuid().ToString('N').Substring(0,8)
$password=[Convert]::ToBase64String([Security.Cryptography.RandomNumberGenerator]::GetBytes(48))+'!aA1'
$credential=[PSCredential]::new($name,(ConvertTo-SecureString $password -AsPlainText -Force))
$password=$null
$account=New-LocalUser -Name $name -Password $credential.Password -PasswordNeverExpires
Add-LocalGroupMember -SID 'S-1-5-32-544' -Member $account
$root=Join-Path $env:RUNNER_TEMP $name
New-Item -ItemType Directory -Path $root | Out-Null
& icacls.exe $root /grant ('*'+$account.SID.Value+':(OI)(CI)F') | Out-Null
$report=Join-Path $root 'split-user.log'
Add-Type -Path (Join-Path $repo 'scripts/installed-shell.cs')
. (Join-Path $repo 'scripts/shell-restart-policy.ps1')
$policy=[Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon',$true)
try {
    $uac=Get-ItemProperty 'HKLM:/SOFTWARE/Microsoft/Windows/CurrentVersion/Policies/System'
    Write-Output ('EnableLUA='+$uac.EnableLUA+'; FilterAdministratorToken='+$uac.FilterAdministratorToken)
    Invoke-ShellRestartPolicyLease -Registry $policy -Body {
        $shell=[AtlasInstalledShell]::ShellPid()
        if($shell){[AtlasInstalledShell]::StopVerifiedShell($shell)}
        $command='-NoProfile -ExecutionPolicy Bypass -File "'+$PSCommandPath+'" -Worker -Repository "'+$repo+'" -Executable "'+$exe+'" -Report "'+$report+'"'
        $worker=Start-Process -FilePath "$env:WINDIR/System32/WindowsPowerShell/v1.0/powershell.exe" -Credential $credential -LoadUserProfile -ArgumentList $command -PassThru -WindowStyle Hidden
        try {
            if(-not $worker.WaitForExit(60000)){throw 'Normal user experiment timed out'}
            if(Test-Path -LiteralPath $report){Get-Content -LiteralPath $report | Write-Output}
            if($worker.ExitCode -ne 0){throw "Normal user experiment exit=$($worker.ExitCode)"}
        } finally {if(-not $worker.HasExited){$worker.Kill();$worker.WaitForExit()};$worker.Dispose()}
    }
} finally {
    $policy.Dispose()
    Remove-LocalUser -SID $account.SID
    $credential=$null
    if(-not [AtlasInstalledShell]::ShellPid()){Start-Process -FilePath "$env:WINDIR/explorer.exe" -WindowStyle Hidden}
}
