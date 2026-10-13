param(
    [Parameter(Mandatory=$true)][string]$PayloadInstaller,
    [Parameter(Mandatory=$true)][string]$BuildId,
    [Parameter(Mandatory=$true)][string]$TargetDirectory,
    [string]$Version,
    [switch]$PassThru
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
if (-not $Version) { $Version = (Get-Content (Join-Path $repo 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version }
$target = (Resolve-Path -LiteralPath $TargetDirectory).Path
$inputInstaller = (Resolve-Path -LiteralPath $PayloadInstaller).Path
$work = Join-Path $repo ('temp/transactional-installer-' + [guid]::NewGuid().ToString('N'))
$payload = Join-Path $work 'payload'
New-Item -ItemType Directory -Path $payload -Force | Out-Null
$sevenZip = Join-Path ${env:ProgramFiles} '7-Zip/7z.exe'
$nsis = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'tauri/NSIS/makensis.exe'
if (-not (Test-Path -LiteralPath $sevenZip) -or -not (Test-Path -LiteralPath $nsis)) { throw 'Pinned NSIS and 7-Zip are required' }
& $sevenZip x $inputInstaller "-o$payload" -y | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Payload extraction failed' }
foreach ($name in @('$PLUGINSDIR','bootstrap.exe','bootstrapc.exe','AtlasUpdater.exe')) {
    $path = [IO.Path]::GetFullPath((Join-Path $payload $name))
    if (-not $path.StartsWith($payload + [IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw 'Cleanup escaped the private payload directory' }
    if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Recurse -Force }
}
Copy-Item -LiteralPath (Join-Path $payload 'Atlas.exe') -Destination (Join-Path $payload 'Atlas.Service.exe')
if ((Get-Item -LiteralPath (Join-Path $payload 'Atlas.exe')).VersionInfo.ProductVersion -ne $Version) { throw 'UI executable version differs from installer version' }
$files = @(Get-ChildItem -LiteralPath $payload -Recurse -File | Sort-Object FullName | ForEach-Object {
    [ordered]@{ path=$_.FullName.Substring($payload.Length+1).Replace('\','/'); size=$_.Length; sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
})
$receipt = Join-Path $work 'install-receipt.json'
$identity = [ordered]@{ id="$Version-$BuildId"; version=$Version; build=$BuildId; files=$files; absent=$false }
[IO.File]::WriteAllText($receipt,($identity | ConvertTo-Json -Depth 5),[Text.UTF8Encoding]::new($false))
. (Join-Path $PSScriptRoot 'initialize-msvc.ps1')
$previousReceipt = $env:ATLAS_INSTALL_RECEIPT
try {
    $env:CARGO_INCREMENTAL='0'
    $env:CARGO_TARGET_DIR=$target
    $env:ATLAS_BUILD_ID=$BuildId
    $env:ATLAS_INSTALL_RECEIPT=$receipt
    & cargo build --locked --offline --release --manifest-path (Join-Path $repo 'src-tauri/Cargo.toml') --bin AtlasUpdater *> (Join-Path $work 'native-build.log')
    if ($LASTEXITCODE -ne 0) { throw "Receipt-bound native build failed; see $work/native-build.log" }
} finally { $env:ATLAS_INSTALL_RECEIPT=$previousReceipt }
Copy-Item -LiteralPath (Join-Path $target 'release/AtlasUpdater.exe') -Destination (Join-Path $payload 'AtlasUpdater.exe')
function ConvertTo-NsisLiteral([string]$Value) { return $Value.Replace('$','$$').Replace('"','$\"') }
$output = Join-Path $work "Atlas-Setup-$Version.exe"
$template = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'transactional-installer.nsi'))
$template = $template.Replace('@VERSION@',$Version).Replace('@OUTPUT@',(ConvertTo-NsisLiteral $output)).Replace('@PAYLOAD@',(ConvertTo-NsisLiteral $payload))
$source = Join-Path $work 'installer.nsi'
[IO.File]::WriteAllText($source,$template,[Text.UTF8Encoding]::new($true))
& $nsis /V3 $source *> (Join-Path $work 'nsis-build.log')
if ($LASTEXITCODE -ne 0) { throw "Transactional installer build failed; see $work/nsis-build.log" }
[ordered]@{ version=$Version; build=$BuildId; installer=$output; sha256=(Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash.ToLowerInvariant(); receipt=$receipt; payload=$payload } |
    ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $work 'build.json') -Encoding utf8
if ($PassThru) { [pscustomobject]@{Installer=$output;Payload=$payload;Receipt=$receipt;Evidence=$work} }
else { Write-Output "Installer: $output"; Write-Output "Evidence: $work" }
