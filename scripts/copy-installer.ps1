$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$version = (Get-Content -LiteralPath (Join-Path $repo 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version
$label = if ($version -match '^1\.0\.0-beta\.(\d+)$') { "Beta $($Matches[1])" } else { $version }
$source = Join-Path $repo "src-tauri/target/release/bundle/nsis/Atlas_${version}_x64-setup.exe"
$destination = Join-Path $repo "Atlas $label Setup.exe"
Copy-Item -LiteralPath $source -Destination $destination -Force
if (Test-Path -LiteralPath "$source.sig") {
    Copy-Item -LiteralPath "$source.sig" -Destination "$destination.sig" -Force
}
Write-Output $destination
