param([string]$TargetDirectory=$env:CARGO_TARGET_DIR)
$ErrorActionPreference='Stop'
# Standalone fixture only: no Atlas settings, installation, service or network.
$repo=Split-Path $PSScriptRoot -Parent
. (Join-Path $PSScriptRoot 'initialize-msvc.ps1')
if (-not $TargetDirectory) {$TargetDirectory='src-tauri/target'}
$target=if ([IO.Path]::IsPathRooted($TargetDirectory)) {[IO.Path]::GetFullPath($TargetDirectory)} else {[IO.Path]::GetFullPath((Join-Path $repo $TargetDirectory))}
$previousTarget=$env:CARGO_TARGET_DIR
try {
    $env:CARGO_TARGET_DIR=$target
    & cargo build --locked --offline --manifest-path (Join-Path $PSScriptRoot 'tray-protocol-fixture/Cargo.toml')
    if ($LASTEXITCODE -ne 0) {throw 'Isolated tray fixture build failed'}
} finally {$env:CARGO_TARGET_DIR=$previousTarget}
$exe=Join-Path $target 'debug/atlas-tray-protocol-fixture.exe'
$root=Join-Path $repo ('temp/tray-protocol-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$process=Start-Process -FilePath $exe -ArgumentList ('"'+$root+'"') -PassThru -WindowStyle Hidden -RedirectStandardError (Join-Path $root 'stderr.log')
$null=$process.Handle
try {
    $deadline=[DateTime]::UtcNow.AddSeconds(5)
    while (-not (Test-Path (Join-Path $root 'ready.txt')) -and [DateTime]::UtcNow -lt $deadline) {Start-Sleep -Milliseconds 50}
    Get-Content (Join-Path $root 'ready.txt') | Write-Output
    # Exercise exactly the native implementation used by installed acceptance,
    # against this fixture's known child PID; never bypass its installed guard.
    $source=Get-Content (Join-Path $PSScriptRoot 'invoke-installed-tray-restart.ps1') -Raw
    $native=[regex]::Match($source,"@'\r?\n([\s\S]*?)\r?\n'@").Groups[1].Value
    if (-not $native) {throw 'Native tray implementation missing'}
    Add-Type -TypeDefinition $native
    [AtlasTrayAcceptance]::Open([uint32]$process.Id)
    $deadline=[DateTime]::UtcNow.AddSeconds(5)
    do {
        $found=[AtlasTrayAcceptance]::HasMenuItem([uint32]$process.Id,'Перезагрузить')
        if ($found) {break}
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if (-not $found) {throw ('Fixture native popup unavailable: '+[AtlasTrayAcceptance]::Diagnostic)}
    # Negative checks run only after the actual popup has been found.
    if ([AtlasTrayAcceptance]::HasMenuItem([uint32]$PID,'Перезагрузить')) {throw 'Wrong popup owner accepted'}
    if ([AtlasTrayAcceptance]::HasMenuItem([uint32]$process.Id,'not-a-fixture-action')) {throw 'Wrong popup action accepted'}
    if ([AtlasTrayAcceptance]::HasMenuItem([uint32]$process.Id,'Disabled')) {throw 'Disabled native command accepted'}
    if (-not [AtlasTrayAcceptance]::TryClickMenu([uint32]$process.Id,'Перезагрузить')) {throw ('Native popup changed before input: '+[AtlasTrayAcceptance]::Diagnostic)}
    if (-not $process.WaitForExit(5000)) {throw 'Native menu click did not reach the fixture handler'}
    $process.WaitForExit()
    if ($process.ExitCode -ne 0 -or (Get-Content (Join-Path $root 'menu.txt') -Raw).Trim() -ne 'restart') {throw 'Fixture did not acknowledge exactly the native restart action'}
    if (-not (Test-Path -LiteralPath (Join-Path $root 'tray-click.txt'))) {throw 'Missing actual tray callback evidence'}
    Write-Output "PASS: native popup owner/label/enabled state/hit-test, wrong-owner/wrong-label/disabled rejection, actual input and callback acknowledgement; evidence=$root"
} catch {
    $_ | Out-String | Set-Content -LiteralPath (Join-Path $root 'failure.log')
    throw
} finally {
    if ('AtlasTrayAcceptance' -as [type]) {
        [AtlasTrayAcceptance]::Diagnostic | Set-Content -LiteralPath (Join-Path $root 'native-menu.log')
        [AtlasTrayAcceptance]::RestoreCursor()
    }
    if (-not $process.HasExited) {$process.Kill();$null=$process.WaitForExit(2000)}
    $process.Dispose()
}
