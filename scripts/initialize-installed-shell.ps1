param([Parameter(Mandatory=$true)][string]$InstallRoot)
$ErrorActionPreference='Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Disposable GitHub Windows runner required' }
$root=(Resolve-Path -LiteralPath $InstallRoot).Path.TrimEnd('\')+'\'
$allowed=[IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')+'\'
if (-not $root.StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase)) { throw 'Shell fixture requires an installation under RUNNER_TEMP' }
Add-Type -Path (Join-Path $PSScriptRoot 'installed-shell.cs')
$desktop=@(Get-CimInstance Win32_Process -Filter "Name='Atlas.exe'" | Where-Object {
    $_.ExecutablePath -and $_.ExecutablePath.StartsWith($root,[StringComparison]::OrdinalIgnoreCase) -and $_.CommandLine -notmatch '--type='
})
if ($desktop.Count -ne 1) { throw 'Shell fixture needs exactly one installed desktop owner' }
$shell=[AtlasInstalledShell]::ShellPid()
$appLevel=[AtlasInstalledShell]::Integrity([uint32]$desktop[0].ProcessId)
$shellLevel=if($shell){[AtlasInstalledShell]::Integrity($shell)}else{0}
Write-Output "Runner desktop integrity=$appLevel; shell integrity=$shellLevel"
if ($appLevel -ne 8192) { throw 'Installed Atlas must run at medium integrity' }
if ($shell -and $shellLevel -eq $appLevel) { return }
# Starting Explorer from an elevated CI step gives the notification area a
# different integrity level from the installed desktop. Use that desktop's
# existing limited token; never elevate Atlas to make the test pass.
$key='HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
$old=(Get-ItemProperty -LiteralPath $key -Name AutoRestartShell -ErrorAction SilentlyContinue).AutoRestartShell
try {
    Set-ItemProperty -LiteralPath $key -Name AutoRestartShell -Value 0 -Type DWord
    if ($shell) { [AtlasInstalledShell]::StopVerifiedShell($shell) }
    [AtlasInstalledShell]::Launch([uint32]$desktop[0].ProcessId)
    $deadline=[DateTime]::UtcNow.AddSeconds(20)
    do {
        $shell=[AtlasInstalledShell]::ShellPid()
        if ($shell -and [AtlasInstalledShell]::Integrity($shell) -eq $appLevel) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    if (-not $shell -or [AtlasInstalledShell]::Integrity($shell) -ne $appLevel) { throw 'Runner notification area did not start at the desktop integrity level' }
    Write-Output 'PASS: disposable notification area and installed Atlas both run at medium integrity'
} finally {
    if ($null -eq $old) { Remove-ItemProperty -LiteralPath $key -Name AutoRestartShell -ErrorAction SilentlyContinue }
    else { Set-ItemProperty -LiteralPath $key -Name AutoRestartShell -Value $old -Type DWord }
}
