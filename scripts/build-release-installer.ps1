param(
    [string]$TargetDirectory = 't/release-build',
    [string]$OutputDirectory = 'artifacts/release',
    [string]$BuildId = $env:ATLAS_BUILD_ID,
    [switch]$Clean,
    [switch]$Signed
)
$ErrorActionPreference='Stop'
$repo=Split-Path $PSScriptRoot -Parent
Set-Location -LiteralPath $repo
$target=[IO.Path]::GetFullPath((Join-Path $repo $TargetDirectory))
$output=[IO.Path]::GetFullPath((Join-Path $repo $OutputDirectory))
if ($Clean -and (Test-Path -LiteralPath $target)) { throw 'Clean release requires a new empty target directory; existing artifacts are preserved' }
if (-not $BuildId) { $BuildId='local-'+(Get-Date -Format yyyyMMdd-HHmmss) }
if ($BuildId -notmatch '^[a-zA-Z0-9_.-]{1,100}$') {throw 'Invalid build identifier'}
if ($Signed -and -not $env:TAURI_SIGNING_PRIVATE_KEY) {throw 'Signed release requires the configured signing environment'}
New-Item -ItemType Directory -Path $target,$output -Force | Out-Null
$version=(Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json).version
if ($version -notmatch '^\d+\.\d+\.\d+$') {throw 'Release installer requires a stable Atlas version'}
. (Join-Path $PSScriptRoot 'initialize-msvc.ps1')
$env:CARGO_TARGET_DIR=$target
$env:CARGO_INCREMENTAL='0'
$env:ATLAS_BUILD_ID=$BuildId
Remove-Item Env:ATLAS_INSTALL_RECEIPT -ErrorAction SilentlyContinue
$override=Join-Path $output 'payload-config.json'
'{"bundle":{"createUpdaterArtifacts":false}}' | Set-Content -LiteralPath $override -Encoding ascii
# The intermediate Tauri installer remains guarded against direct upgrade.
# Only the receipt-bound transactional wrapper is delivered or signed.
& npm.cmd run tauri -- build --bundles nsis --config $override -- --locked
if ($LASTEXITCODE -ne 0) {throw 'Release payload build failed'}
$payloadInstaller=Join-Path $target "release/bundle/nsis/Atlas_${version}_x64-setup.exe"
$built=& (Join-Path $PSScriptRoot 'build-transactional-installer.ps1') -PayloadInstaller $payloadInstaller -BuildId $BuildId -TargetDirectory $target -Version $version -PassThru
$installer=Join-Path $output "Atlas_${version}_x64-setup.exe"
Copy-Item -LiteralPath $built.Installer -Destination $installer
& (Join-Path $PSScriptRoot 'verify-transactional-installer.ps1') -Installer $installer -ExpectedPayload $built.Payload -EvidenceDirectory (Join-Path $output 'verification') -ExpectedVersion $version
$verification=Join-Path $output 'verification/verification.json'
if ($Signed) {
    & npm.cmd run tauri -- signer sign $installer
    if ($LASTEXITCODE -ne 0) {throw 'Installer signing failed'}
    & node (Join-Path $PSScriptRoot 'verify-installer.cjs') $installer
    if ($LASTEXITCODE -ne 0) {throw 'Installer signature verification failed'}
    $manifest=Join-Path $output 'update-manifest.json'
    & node (Join-Path $PSScriptRoot 'create-update-manifest.cjs') $installer $verification $manifest
    if ($LASTEXITCODE -ne 0) {throw 'Transactional update manifest generation failed'}
    & npm.cmd run tauri -- signer sign $manifest
    if ($LASTEXITCODE -ne 0) {throw 'Manifest signing failed'}
    & (Join-Path $built.Payload 'AtlasUpdater.exe') --verify-package $manifest "$manifest.sig" $installer
    if ($LASTEXITCODE -ne 0) {throw 'Native signed-package verification failed'}
}
$hash=(Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
"$hash  $([IO.Path]::GetFileName($installer))" | Set-Content -LiteralPath (Join-Path $output 'SHA256SUMS.txt') -Encoding ascii
[ordered]@{version=$version;build=$BuildId;installer=$installer;sha256=$hash;signed=[bool]$Signed;cleanTarget=[bool]$Clean;payload=$built.Payload;verification=$verification} |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $output 'build.json') -Encoding utf8
Write-Output "Verified release installer: $installer"
