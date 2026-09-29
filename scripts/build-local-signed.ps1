param([string]$SigningKeyDirectory = (Join-Path ([Environment]::GetFolderPath('UserProfile')) '.tauri'))
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Set-Location -LiteralPath $repo
. (Join-Path $PSScriptRoot 'initialize-msvc.ps1')
$env:CARGO_TARGET_DIR = Join-Path $repo 't'
$env:CARGO_BUILD_JOBS = '2'
$manifest = Get-Content src-tauri/resources/xray-manifest.json -Raw | ConvertFrom-Json
foreach ($entry in @(@('Atlas.Xray.exe','brandedSha256'),@('xray-assets/geoip.dat','geoipSha256'),@('xray-assets/geosite.dat','geositeSha256'))) {
    if ((Get-FileHash -LiteralPath (Join-Path "$repo/src-tauri/resources" $entry[0]) -Algorithm SHA256).Hash -ne $manifest.($entry[1])) { throw "Pinned Xray resource differs: $($entry[0])" }
}
$cef = foreach ($profile in @('release','debug')) {
    Get-ChildItem -LiteralPath (Join-Path $env:CARGO_TARGET_DIR "$profile/build") -Directory -Filter 'cef-dll-sys-*' |
        ForEach-Object { Join-Path $_.FullName 'out/cef_windows_x86_64' } |
        Where-Object { (Test-Path (Join-Path $_ 'archive.json')) -and (Test-Path (Join-Path $_ 'libcef.dll')) }
}
$env:CEF_PATH = $cef | Select-Object -First 1
if (-not $env:CEF_PATH) { throw 'Verified local CEF distribution not found' }
$inputs = @('src','src-tauri/src','src-tauri/resources') | ForEach-Object {
    Get-ChildItem -LiteralPath (Join-Path $repo $_) -File -Recurse |
        ForEach-Object { $_.FullName.Substring($repo.Length) + ':' + (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }
}
$inputs += @('package.json','package-lock.json','src-tauri/Cargo.toml','src-tauri/Cargo.lock','src-tauri/tauri.conf.json') | ForEach-Object { $_ + ':' + (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash }
$hash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes(($inputs | Sort-Object) -join "`n"))).ToLowerInvariant()
$env:ATLAS_BUILD_ID = 'happ-local-' + $hash.Substring(0,12)
$config = Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json
$keyDirectory = $SigningKeyDirectory
if ((Get-Content (Join-Path $keyDirectory 'atlas-updater.key.pub') -Raw).Trim() -ne $config.plugins.updater.pubkey) { throw 'Local signing key does not match updater trust' }
try {
    $env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content (Join-Path $keyDirectory 'atlas-updater.key') -Raw).Trim()
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = (Get-Content (Join-Path $keyDirectory 'atlas-updater.password.txt') -Raw).TrimEnd("`r","`n")
    & npm.cmd run tauri -- build --bundles nsis
    if ($LASTEXITCODE -ne 0) { throw 'Local installer build failed' }
} finally {
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
}
$file = Join-Path $env:CARGO_TARGET_DIR "release/bundle/nsis/Atlas_$($config.version)_x64-setup.exe"
& node (Join-Path $PSScriptRoot 'verify-installer.cjs') $file
if ($LASTEXITCODE -ne 0) { throw 'Updater signature verification failed' }
Write-Output "Build ID: $env:ATLAS_BUILD_ID"
