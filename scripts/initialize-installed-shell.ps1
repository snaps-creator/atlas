param([Parameter(Mandatory=$true)][string]$InstallRoot,[scriptblock]$Action)
$ErrorActionPreference='Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Disposable GitHub Windows runner required' }
$root=(Resolve-Path -LiteralPath $InstallRoot).Path.TrimEnd('\')+'\'
$allowed=[IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')+'\'
if (-not $root.StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase)) { throw 'Shell fixture requires an installation under RUNNER_TEMP' }
if (-not $Action) { throw 'Installed shell fixture requires a scoped test action' }
$installedBody=$Action
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
# Explorer supplies a read-only registered-icon rectangle. Real input goes to
# Atlas's native menu, whose owner and integrity are checked at the click site.
# Replacing the runner's shell with Atlas's restricted token destroys this
# independent desktop fixture on hosted Windows; retain the existing shell.
if (-not $shell) { throw 'Runner notification area unavailable' }
[AtlasInstalledShell]::VerifyShell($shell)
Write-Output "PASS: existing runner notification area PID=$shell verified; installed Atlas remains medium; input integrity checked on the native menu owner"
try { & $installedBody }
finally { Write-Output ('Shell fixture completion: '+[AtlasInstalledShell]::ShellDiagnostic()) }
