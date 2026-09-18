$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    npm.cmd ci
    if ($LASTEXITCODE) { throw 'npm ci failed' }
    npm.cmd test
    if ($LASTEXITCODE) { throw 'Frontend tests failed' }
    cargo test --manifest-path src-tauri/Cargo.toml --lib
    if ($LASTEXITCODE) { throw 'Rust tests failed' }
    npm.cmd run tauri build
    if ($LASTEXITCODE) { throw 'Installer build failed' }
} finally { Pop-Location }
