param([Parameter(Mandatory=$true)][ValidateSet('UiSmoke','ManualRestart','Overflow')][string]$Action,[string]$Executable,[Parameter(Mandatory=$true)][string]$EvidencePath)
$ErrorActionPreference='Stop'
if($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows'){throw 'Disposable GitHub Windows runner required'}
$allowed=[IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')+'\'
if(-not [IO.Path]::GetFullPath($EvidencePath).StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase)){throw 'Desktop evidence must stay under RUNNER_TEMP'}
Add-Type -Path (Join-Path $PSScriptRoot 'installed-shell.cs')
$identity=[Security.Principal.WindowsIdentity]::GetCurrent()
if([AtlasInstalledShell]::Integrity([uint32]$PID) -ne 8192 -or [Security.Principal.WindowsPrincipal]::new($identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)){throw 'Desktop acceptance requires an ordinary medium user'}
$shell=[AtlasInstalledShell]::ShellPid();[AtlasInstalledShell]::VerifyShell($shell)
if([AtlasInstalledShell]::Integrity($shell) -ne 8192){throw 'Desktop acceptance requires the same ordinary shell'}
if($Action -ne 'Overflow') {
    $Executable=(Resolve-Path -LiteralPath $Executable).Path
    if(-not $Executable.StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($Executable) -ne 'Atlas.exe'){throw 'Installed desktop must be Atlas under RUNNER_TEMP'}
}
Start-Transcript -LiteralPath $EvidencePath -Force | Out-Null
try {
    switch($Action) {
        'UiSmoke' { & (Join-Path $PSScriptRoot 'test-ui-startup.ps1') -Executable $Executable }
        'ManualRestart' { $child=Start-Process -FilePath $Executable -ArgumentList '--manual-restart' -PassThru -WindowStyle Hidden;Write-Output ('Started real manual restart at medium integrity; PID='+$child.Id);$child.Dispose() }
        'Overflow' { & (Join-Path $PSScriptRoot 'show-installed-tray-overflow.ps1') }
    }
} finally {Stop-Transcript | Out-Null}
