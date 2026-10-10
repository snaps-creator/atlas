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
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    [AtlasTrayAcceptance]::Open([uint32]$process.Id)
    $condition=[System.Windows.Automation.AndCondition]::new(
        [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty,'Перезагрузить'),
        [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ProcessIdProperty,$process.Id))
    $deadline=[DateTime]::UtcNow.AddSeconds(5)
    do {
        $item=[System.Windows.Automation.AutomationElement]::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Descendants,$condition)
        if ($item) {break}
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if (-not $item -or -not $item.Current.IsEnabled -or $item.Current.ControlType -ne [System.Windows.Automation.ControlType]::MenuItem) {throw 'Fixture native menu item unavailable'}
    $bounds=$item.Current.BoundingRectangle
    if ($bounds.IsEmpty -or $bounds.Width -le 0 -or $bounds.Height -le 0) {throw 'Fixture item has no native input target'}
    [AtlasTrayAcceptance]::Click([int]($bounds.Left+$bounds.Width/2),[int]($bounds.Top+$bounds.Height/2))
    if (-not $process.WaitForExit(5000)) {throw 'Native menu click did not reach the fixture handler'}
    $process.WaitForExit()
    if ($process.ExitCode -ne 0 -or (Get-Content (Join-Path $root 'menu.txt') -Raw).Trim() -ne 'restart') {throw 'Fixture did not acknowledge exactly the native restart action'}
    Write-Output "PASS: registered native tray, actual popup click, callback acknowledgement and normal fixture exit; evidence=$root"
} finally {
    if ('AtlasTrayAcceptance' -as [type]) {[AtlasTrayAcceptance]::RestoreCursor()}
    if (-not $process.HasExited) {Stop-Process -Id $process.Id -Force}
}
