$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$version = (Get-Content -LiteralPath (Join-Path $repo 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version
$label = if ($version -match '^1\.0\.0-beta\.(\d+(?:\.\d+)*)$') { "Beta $($Matches[1])" } else { $version }
$source = Join-Path $repo "src-tauri/target/release/bundle/nsis/Atlas_${version}_x64-setup.exe"
$destination = Join-Path $repo "Atlas $label Setup.exe"
if (-not (Test-Path -LiteralPath "$source.sig")) { throw 'Updater signature is missing; refusing to copy an unsigned installer.' }
Push-Location $repo
try {
    & node (Join-Path $PSScriptRoot 'verify-installer.cjs') $source
    if ($LASTEXITCODE -ne 0) { throw 'Installer signature verification failed.' }
    Copy-Item -LiteralPath $source -Destination $destination -Force
    Copy-Item -LiteralPath "$source.sig" -Destination "$destination.sig" -Force
} finally { Pop-Location }
Write-Output $destination
