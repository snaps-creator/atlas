param([switch]$Signed)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Push-Location -LiteralPath $repo
try {
    . (Join-Path $PSScriptRoot 'initialize-msvc.ps1')
    $build = Get-Date -Format 'yyyyMMdd-HHmmss'
    $env:CARGO_TARGET_DIR = Join-Path $repo "t/tests-$build"
    $env:RUST_TEST_THREADS = '1'
    & npm.cmd ci
    if ($LASTEXITCODE -ne 0) { throw 'Dependency restore failed' }
    & cargo fetch --locked --manifest-path src-tauri/Cargo.toml
    if ($LASTEXITCODE -ne 0) { throw 'Rust dependency restore failed' }
    & npm.cmd test
    if ($LASTEXITCODE -ne 0) { throw 'Frontend tests failed' }
    & node --test scripts/*.check.cjs
    if ($LASTEXITCODE -ne 0) { throw 'Build/update contract tests failed' }
    & cargo test --locked --manifest-path src-tauri/Cargo.toml --lib --bin AtlasMaintenance --bin AtlasUpdater
    if ($LASTEXITCODE -ne 0) { throw 'Native tests failed' }
    & (Join-Path $PSScriptRoot 'build-release-installer.ps1') -Clean -Signed:$Signed -TargetDirectory "t/release-$build" -OutputDirectory "artifacts/release-$build"
} finally { Pop-Location }
