param(
    [Parameter(Mandatory=$true)][string]$Payload,
    [Parameter(Mandatory=$true)][string]$Installer,
    [string]$ExpectedVersion
)
$ErrorActionPreference='Stop'
$repo=Split-Path $PSScriptRoot -Parent
if (-not $ExpectedVersion) {$ExpectedVersion=(Get-Content (Join-Path $repo 'src-tauri/tauri.conf.json') -Raw|ConvertFrom-Json).version}
$root=(Resolve-Path -LiteralPath $Payload).Path
function Require-File([string]$Name) {
    $path=Join-Path $root $Name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {throw "Release contract: required file missing: $Name"}
    return $path
}
function Assert-X64([string]$Path) {
    $stream=[IO.File]::OpenRead($Path);$reader=[IO.BinaryReader]::new($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5a4d) {throw "Not a PE executable: $Path"}
        $stream.Position=0x3c;$offset=$reader.ReadUInt32();$stream.Position=$offset
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) {throw "Release contract: executable is not x64: $Path"}
    } finally {$reader.Dispose();$stream.Dispose()}
}
foreach ($name in @('Atlas.exe','Atlas.Service.exe','AtlasUpdater.exe','AtlasMaintenance.exe')) {
    $path=Require-File $name
    if ((Get-Item -LiteralPath $path).VersionInfo.ProductVersion -ne $ExpectedVersion) {throw "Release contract: wrong product version: $name; expected $ExpectedVersion"}
    Assert-X64 $path
}
# The stable installed launcher is a copy of the native AtlasUpdater.exe.
# A distinct AtlasLauncher.exe is deliberately not part of this contract.
if ((Get-FileHash (Join-Path $root 'Atlas.exe')).Hash -ne (Get-FileHash (Join-Path $root 'Atlas.Service.exe')).Hash) {throw 'Release contract: app/service identity mismatch'}
foreach ($name in @('libcef.dll','chrome_elf.dll','libEGL.dll','libGLESv2.dll','d3dcompiler_47.dll','dxcompiler.dll','dxil.dll','vk_swiftshader.dll','vulkan-1.dll')) {Assert-X64 (Require-File $name)}
foreach ($name in @('chrome_100_percent.pak','chrome_200_percent.pak','resources.pak','icudtl.dat','v8_context_snapshot.bin','vk_swiftshader_icd.json','locales/en-US.pak','resources/geoip-manifest.json','resources/xray-manifest.json','resources/LICENSE-geoip','resources/LICENSE-mihomo','resources/LICENSE-xray')) { $null=Require-File $name }
$core=Get-Content (Join-Path $repo 'src-tauri/resources/atlas-core-manifest.json') -Raw|ConvertFrom-Json
$xray=Get-Content (Join-Path $repo 'src-tauri/resources/xray-manifest.json') -Raw|ConvertFrom-Json
foreach ($entry in @(@('resources/Atlas.Core.exe',$core.brandedSha256),@('resources/Atlas.Xray.exe',$xray.brandedSha256),@('resources/xray-assets/geoip.dat',$xray.geoipSha256),@('resources/xray-assets/geosite.dat',$xray.geositeSha256))) {
    $path=Require-File $entry[0]
    if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $entry[1]) {throw "Release contract: pinned resource mismatch: $($entry[0])"}
    if ($path.EndsWith('.exe')) {Assert-X64 $path}
}
if ((Get-Item -LiteralPath $Installer).VersionInfo.ProductVersion -ne $ExpectedVersion) {throw 'Release contract: installer version mismatch'}
# Native read-only receipt verification binds every extracted file to the build,
# including the launcher/updater binary. No installer or service is started.
$probe=Join-Path $env:TEMP ('atlas-contract-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $probe | Out-Null
try {
    $child=Start-Process -FilePath (Join-Path $root 'AtlasUpdater.exe') -ArgumentList ('--verify-local-payload "'+$root+'"') -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $probe 'result.json') -RedirectStandardError (Join-Path $probe 'error.txt')
    $null=$child.Handle
    if (-not $child.WaitForExit(30000)) {$child.Kill();throw 'Read-only release contract verification timed out'}
    $child.WaitForExit()
    if ($child.ExitCode -ne 0) {throw 'Release contract: embedded payload receipt rejected files'}
    $receipt=Get-Content (Join-Path $probe 'result.json') -Raw|ConvertFrom-Json
    if ($receipt.verified -ne $true -or $receipt.version -ne $ExpectedVersion) {throw 'Release contract: native verifier returned incompatible metadata'}
    Write-Output "PASS: transactional release contract $ExpectedVersion; app/service/updater-launcher, x64 runtime, pinned cores, metadata and receipt."
} finally {
    $resolved=[IO.Path]::GetFullPath($probe)
    $tempRoot=[IO.Path]::GetFullPath($env:TEMP).TrimEnd('\')+'\'
    if (-not $resolved.StartsWith($tempRoot,[StringComparison]::OrdinalIgnoreCase) -or (Split-Path $resolved -Leaf) -notlike 'atlas-contract-*') {throw 'Unsafe contract evidence cleanup'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}