# Supply the existing Tauri signing environment explicitly. Never search a user
# profile for private keys, and never generate a replacement release identity.
$ErrorActionPreference = 'Stop'
if (-not $env:TAURI_SIGNING_PRIVATE_KEY) { throw 'TAURI_SIGNING_PRIVATE_KEY must be configured before a signed build' }
& (Join-Path $PSScriptRoot 'build.ps1') -Signed
