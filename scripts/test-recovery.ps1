param([string]$Filter = '')
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'initialize-msvc.ps1')
$env:RUST_TEST_THREADS = '1'
& cargo test --offline --manifest-path (Join-Path $PSScriptRoot '../src-tauri/Cargo.toml') --lib --bin AtlasMaintenance $Filter -- --nocapture
exit $LASTEXITCODE
