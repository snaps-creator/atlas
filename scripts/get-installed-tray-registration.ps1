param([Parameter(Mandatory=$true)][int]$ProcessId,[Parameter(Mandatory=$true)][string]$Executable,[Parameter(Mandatory=$true)][string]$OutputPath)
$ErrorActionPreference='Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {throw 'Disposable GitHub Windows runner required'}
$allowed=[IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')+'\'
$image=(Resolve-Path -LiteralPath $Executable).Path
if (-not $image.StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($image) -ne 'Atlas.exe') {throw 'Registration query requires the installed runner desktop image'}
if (-not [IO.Path]::GetFullPath($OutputPath).StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase)) {throw 'Tray registration proof must stay under RUNNER_TEMP'}
$owner=Get-CimInstance Win32_Process -Filter "ProcessId=$ProcessId"
if (-not $owner -or $owner.ExecutablePath -ne $image -or $owner.CommandLine -match '--type=') {throw 'Registration query owner does not match the installed desktop'}
Add-Type -Path (Join-Path $PSScriptRoot 'installed-shell.cs')
$shell=[AtlasInstalledShell]::ShellPid()
if (-not $shell) {throw 'Runner notification area unavailable for registration query'}
[AtlasInstalledShell]::VerifyShell($shell)
$callerLevel=[AtlasInstalledShell]::Integrity([uint32]$PID)
$shellLevel=[AtlasInstalledShell]::Integrity($shell)
if ($callerLevel -lt $shellLevel -or [AtlasInstalledShell]::Integrity([uint32]$ProcessId) -ne 8192) {throw 'Parent shell query context or installed desktop integrity is incorrect'}
$source=Get-Content (Join-Path $PSScriptRoot 'invoke-installed-tray-restart.ps1') -Raw
$native=[regex]::Match($source,"@'\r?\n([\s\S]*?)\r?\n'@").Groups[1].Value
if (-not $native) {throw 'Native tray implementation missing'}
Add-Type -TypeDefinition $native
# This phase is read-only: no cursor movement, callback, input or registration.
$registration=[AtlasTrayAcceptance]::QueryRegisteredIcon([uint32]$ProcessId)
$json=$registration | ConvertTo-Json -Compress
[IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputPath),$json,[Text.UTF8Encoding]::new($false))
Write-Output "PASS: actual registered icon queried in parent context helper=$callerLevel shell=$shellLevel proof=$json"
