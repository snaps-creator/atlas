param(
    [Parameter(Mandatory=$true)][string]$NodeExecutable,
    [switch]$Unsigned,
    [switch]$ReleaseTests
)
$ErrorActionPreference = 'Stop'
if (-not $Unsigned) { throw 'Local verification requires -Unsigned. Production signing is CI-only; supply a signed CI artifact to the package verifier.' }
$repo = Split-Path $PSScriptRoot -Parent
Set-Location -LiteralPath $repo
$env:PATH = (Split-Path $NodeExecutable -Parent) + ';' + $env:PATH
if ((& node --version) -notmatch '^v22\.') { throw 'Use Node 22, matching release CI' }
if ((& node --version) -ne 'v22.23.3' -or (& npm.cmd --version) -ne '11.16.0') { throw 'Node/npm versions must match release CI exactly' }
. (Join-Path $PSScriptRoot 'initialize-msvc.ps1')
$run = 'p0-' + (Get-Date -Format 'yyyyMMdd-HHmmss')
$evidence = Join-Path $repo "temp/update-build/$run"
$target = Join-Path $repo "t/$run"
if (Test-Path -LiteralPath $target) { throw 'Clean build target already exists' }
New-Item -ItemType Directory -Path $evidence,$target | Out-Null
# Windows' global npm.cmd prefers its adjacent node.exe, ignoring PATH.
# Make nested beforeBuildCommand invocations use the same explicitly selected Node.
$npmCli = Join-Path (Split-Path (Get-Command npm.cmd).Source -Parent) 'node_modules/npm/bin/npm-cli.js'
if (-not (Test-Path -LiteralPath $npmCli)) { throw 'npm CLI entrypoint missing' }
$env:ATLAS_BUILD_NPM_CLI = $npmCli
$env:NODE_OPTIONS = '--require "' + (Join-Path $PSScriptRoot 'locked-node.cjs').Replace('\','/') + '"'
'@echo off', 'node "%ATLAS_BUILD_NPM_CLI%" %*' | Set-Content "$evidence/npm.cmd" -Encoding ascii
$env:PATH = $evidence + ';' + $env:PATH
$env:CARGO_TARGET_DIR = $target
$env:CARGO_BUILD_JOBS = '2'
$env:CARGO_INCREMENTAL = '0'
$env:RUST_TEST_THREADS = '1'
$env:ATLAS_BUILD_ID = $run
$inputs = @('package.json','package-lock.json','src-tauri/Cargo.toml','src-tauri/Cargo.lock','src-tauri/tauri.conf.json','src-tauri/build.rs','src-tauri/installer-hooks.nsh','rust-toolchain.toml')
$before = @{}
foreach ($file in $inputs) { $before[$file] = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash }
$before | ConvertTo-Json | Set-Content "$evidence/inputs.json"
git diff --binary | Set-Content "$evidence/source.patch"
git status --short | Set-Content "$evidence/source-status.txt"
$sourceRoots = @('src','src-tauri/src','src-tauri/resources','src-tauri/testdata','scripts','.github')
$sourceFiles = $sourceRoots | ForEach-Object { Get-ChildItem -LiteralPath $_ -Recurse -File }
$sourceFiles | ForEach-Object { @{path=$_.FullName; sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash} } | ConvertTo-Json -Depth 3 | Set-Content "$evidence/source-hashes.json"
try {
    & npm.cmd ci --no-audit --no-fund *> "$evidence/npm-ci.log"
    if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
    & node -e 'const fs=require("fs"); const l=require("./package-lock.json"); for(const n of ["@tauri-apps/cli","@tauri-apps/plugin-updater","@tauri-apps/api"]){const v=JSON.parse(fs.readFileSync("node_modules/"+n+"/package.json")).version;if(v!==l.packages["node_modules/"+n].version)throw Error(n+" differs from lockfile"); console.log(n+"="+v)}' > "$evidence/js-versions.txt"
    if ($LASTEXITCODE -ne 0) { throw 'Installed JS dependencies differ from lockfile' }
    @((& node --version),(& npm.cmd --version),(& rustc -vV),(& cargo --version),(& npm.cmd run tauri -- --version)) | Set-Content "$evidence/toolchain.txt"
    & cargo metadata --locked --filter-platform x86_64-pc-windows-msvc --format-version 1 --manifest-path src-tauri/Cargo.toml > "$evidence/cargo-metadata.json"
    if ($LASTEXITCODE -ne 0) { throw 'Locked Cargo dependency resolution failed' }
    & cargo tree --locked --manifest-path src-tauri/Cargo.toml > "$evidence/cargo-tree.txt"
    if ($LASTEXITCODE -ne 0) { throw 'Cargo tree failed' }
    & cargo tree --locked --manifest-path src-tauri/Cargo.toml -e features > "$evidence/cargo-features.txt"
    if ($LASTEXITCODE -ne 0) { throw 'Cargo features failed' }
    & npm.cmd test *> "$evidence/frontend-tests.log"
    if ($LASTEXITCODE -ne 0) { throw 'Frontend tests failed' }
    # No target cache is reused. Tauri obtains its CEF distribution normally.
    $config = Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json
    $buildArgs = @('run','tauri','--','build','--bundles','nsis')
    if ($Unsigned) {
        '{"bundle":{"createUpdaterArtifacts":false}}' | Set-Content "$evidence/unsigned.json" -Encoding ascii
        $buildArgs += @('--config', "$evidence/unsigned.json")
    }
    & npm.cmd @buildArgs -- --locked *> "$evidence/build.log"
    if ($LASTEXITCODE -ne 0) { throw 'Clean NSIS build failed' }
    & node scripts/capture-update-build.cjs $target $evidence
    if ($LASTEXITCODE -ne 0) { throw 'Build evidence capture failed' }
    $installer = Join-Path $target "release/bundle/nsis/Atlas_$($config.version)_x64-setup.exe"
    if (-not $Unsigned) {
        & node scripts/verify-installer.cjs $installer > "$evidence/signature.json"
        if ($LASTEXITCODE -ne 0) { throw 'Installer signature check failed' }
    }
    $nsis = Join-Path $target 'release/nsis/x64/installer.nsi'
    if (-not (Test-Path -LiteralPath $nsis)) { throw 'Fresh generated NSIS missing' }
    Copy-Item -LiteralPath $nsis -Destination "$evidence/installer.nsi"
    $sevenZip = Join-Path $env:ProgramFiles '7-Zip/7z.exe'
    if (-not (Test-Path -LiteralPath $sevenZip)) { $sevenZip = (Get-Command 7z.exe -ErrorAction Stop).Source }
    $extracted = Join-Path $evidence 'extracted'
    & $sevenZip x $installer "-o$extracted" -y *> "$evidence/extraction.log"
    if ($LASTEXITCODE -ne 0) { throw 'Package extraction failed' }
    & node scripts/verify-package-content.cjs $nsis $extracted $config.version "$evidence/package-content.json"
    if ($LASTEXITCODE -ne 0) { throw 'Package bytes or PE architecture verification failed' }
    Get-ChildItem -LiteralPath $extracted -File -Recurse | Where-Object { $_.Extension -in '.exe','.dll' } | ForEach-Object {
        @{path=$_.FullName;fileVersion=$_.VersionInfo.FileVersion;productVersion=$_.VersionInfo.ProductVersion}
    } | ConvertTo-Json | Set-Content "$evidence/pe-versions.json"
    $isolated = Join-Path $evidence 'native-only'
    New-Item -ItemType Directory -Path $isolated | Out-Null
    foreach ($name in @('AtlasMaintenance','AtlasUpdater')) {
        $exe = Join-Path $isolated "$name.exe"
        Copy-Item -LiteralPath (Join-Path $extracted "$name.exe") -Destination $exe
        $process = Start-Process -FilePath $exe -ArgumentList '--protocol' -WorkingDirectory $isolated -WindowStyle Hidden -PassThru -RedirectStandardOutput "$isolated/$name.json" -RedirectStandardError "$isolated/$name.err"
        if (-not $process.WaitForExit(10000)) { $process.Kill(); throw 'Native helper exceeded protocol deadline' }
        if ($process.ExitCode -ne 0) { throw 'Native helper failed without CEF' }
        $protocol = Get-Content -LiteralPath "$isolated/$name.json" -Raw | ConvertFrom-Json
        if ($protocol.protocol -ne 1 -or $protocol.version -ne $config.version -or $protocol.build -ne $run) { throw 'Native helper protocol/build identity mismatch' }
    }
    $testArgs = @('test','--locked','--manifest-path','src-tauri/Cargo.toml','--lib','--bin','AtlasMaintenance','--bin','AtlasUpdater')
    if ($ReleaseTests) { $testArgs += '--release' }
    & cargo @testArgs *> "$evidence/backend-tests.log"
    if ($LASTEXITCODE -ne 0) { throw 'Backend tests failed' }
    foreach ($file in $inputs) { if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $before[$file]) { throw "Build changed locked input: $file" } }
    foreach ($source in (Get-Content "$evidence/source-hashes.json" -Raw | ConvertFrom-Json)) {
        if ((Get-FileHash -LiteralPath $source.path -Algorithm SHA256).Hash -ne $source.sha256) { throw "Source changed during build: $($source.path)" }
    }
    $afterPaths = @($sourceRoots | ForEach-Object { Get-ChildItem -LiteralPath $_ -Recurse -File } | ForEach-Object FullName | Sort-Object)
    if (Compare-Object @($sourceFiles.FullName | Sort-Object) $afterPaths) { throw 'Source inventory changed during build' }
    $artifactPaths = @($installer, $nsis, (Join-Path $target 'release/Atlas.exe'), (Join-Path $target 'release/AtlasMaintenance.exe'), (Join-Path $target 'release/AtlasUpdater.exe'))
    if (-not $Unsigned) { $artifactPaths += "$installer.sig" }
    $artifacts = $artifactPaths | ForEach-Object {
        $item = Get-Item -LiteralPath $_
        @{ path=$item.FullName; size=$item.Length; sha256=(Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash; fileVersion=$item.VersionInfo.FileVersion }
    }
    @{ build=$run; version=$config.version; target=$target; signed=(-not $Unsigned); artifacts=$artifacts; result='passed' } | ConvertTo-Json -Depth 6 | Set-Content "$evidence/result.json"
} catch {
    @{ build=$run; target=$target; result='failed'; reason=$_.Exception.Message } | ConvertTo-Json | Set-Content "$evidence/result.json"
    throw
} finally {
    Write-Output "Build evidence: $evidence"
}
