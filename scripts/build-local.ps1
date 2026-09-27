param([switch]$SkipTests, [string]$AcceptanceReport)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$version = '2.0.1-alpha.1'
$name = 'Atlas-Setup-2.0.1-alpha-windows-x64.exe'
$buildStarted = Get-Date
$downloads = $null
if ($AcceptanceReport -and $SkipTests) { throw 'Release cannot skip tests' }
if ($AcceptanceReport) {
    if (-not (Test-Path -LiteralPath $AcceptanceReport -PathType Leaf)) { throw 'Acceptance report not found' }
    $acceptance = Get-Content -LiteralPath $AcceptanceReport -Raw | ConvertFrom-Json
    if ($acceptance.architectureVerified -ne $true -or
        $acceptance.processNamesVerified -ne $true -or
        $acceptance.networkRestored -ne $true -or
        $acceptance.installerLifecycleVerified -ne $true -or
        $acceptance.postExitClean -ne $true -or
        $acceptance.pingParityVerified -ne $true -or
        $acceptance.safeTestsOnly -ne $true) {
        throw 'Acceptance report does not cover every required release gate'
    }
}
if ($AcceptanceReport) {
    $downloadsValue = (Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders' -Name '{374DE290-123F-4565-9164-39C4925E467B}').'{374DE290-123F-4565-9164-39C4925E467B}'
    $downloads = [Environment]::ExpandEnvironmentVariables($downloadsValue)
    if (-not (Test-Path -LiteralPath $downloads -PathType Container)) { throw "Downloads folder unavailable: $downloads" }
}
$manifest = Get-Content -LiteralPath (Join-Path $repo 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json
if ($manifest.version -ne $version -or $manifest.identifier -ne 'net.atlasvpn.desktop') { throw 'Version or application identity mismatch' }
if ((& rustc -vV | Select-String '^host:').ToString() -notmatch 'x86_64-pc-windows-msvc') { throw 'An x64 MSVC Rust toolchain is required' }
$core = Join-Path $repo 'src-tauri/resources/Atlas.Core.exe'
$coreManifest = Get-Content -LiteralPath (Join-Path $repo 'src-tauri/resources/atlas-core-manifest.json') -Raw | ConvertFrom-Json
if ((Get-FileHash -LiteralPath $core -Algorithm SHA256).Hash -ne $coreManifest.brandedSha256) { throw 'Branded core hash mismatch' }
if (-not (Get-Item -LiteralPath $core).VersionInfo.FileDescription.StartsWith('Atlas')) { throw 'Core FileDescription does not start with Atlas' }
$fingerprints = foreach ($item in @('src','src-tauri/src','src-tauri/resources','src-tauri/icons','scripts')) {
    Get-ChildItem -LiteralPath (Join-Path $repo $item) -File -Recurse |
        ForEach-Object { "$($_.FullName.Substring($repo.Length)):$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash)" }
}
$fingerprints += foreach ($item in @('package.json','package-lock.json','index.html','vite.config.ts','tsconfig.json','src-tauri/Cargo.toml','src-tauri/Cargo.lock','src-tauri/tauri.conf.json','src-tauri/build.rs','src-tauri/installer-hooks.nsh')) {
    $path = Join-Path $repo $item
    "$($item):$((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash)"
}
$fingerprintText = ($fingerprints | Sort-Object -CaseSensitive) -join "`n"
$fingerprintBytes = [Text.Encoding]::UTF8.GetBytes($fingerprintText)
$sha256 = [Security.Cryptography.SHA256]::Create()
try {
    $fingerprintHash = [BitConverter]::ToString($sha256.ComputeHash($fingerprintBytes)).Replace('-', '').ToLowerInvariant()
} finally { $sha256.Dispose() }
$env:ATLAS_BUILD_ID = 'alpha2.0.1-' + $fingerprintHash.Substring(0,12)
if ($AcceptanceReport -and $acceptance.buildId -ne $env:ATLAS_BUILD_ID) {
    throw 'Acceptance report belongs to a different source build'
}
Push-Location $repo
try {
    $vsDev = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat'
    if (Test-Path -LiteralPath $vsDev -PathType Leaf) {
        $vsEnvironment = & cmd.exe /d /c "call `"$vsDev`" -arch=amd64 >nul && set"
        if ($LASTEXITCODE -ne 0) { throw 'Visual Studio build environment failed' }
        foreach ($line in $vsEnvironment) {
            if ($line -match '^([^=]+)=(.*)$') {
                [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
            }
        }
    }
    $vsCmake = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake'
    if (Test-Path -LiteralPath (Join-Path $vsCmake 'CMake\bin\cmake.exe') -PathType Leaf) {
        $env:PATH = (Join-Path $vsCmake 'CMake\bin') + ';' + (Join-Path $vsCmake 'Ninja') + ';' + $env:PATH
    }
    $env:CARGO_BUILD_JOBS = '2'
    # CEF's generated C++ object paths exceed MAX_PATH under the normal
    # OneDrive/src-tauri/target hierarchy. Keep the build output shallow.
    $env:CARGO_TARGET_DIR = Join-Path $repo 't'
    & npm.cmd ci
    if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
    if (-not $SkipTests) {
        & npm.cmd test
        if ($LASTEXITCODE -ne 0) { throw 'Frontend tests failed' }
        & cargo test --manifest-path src-tauri/Cargo.toml --lib
        if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed' }
    }
    # Tauri CLI otherwise redirects CEF into the user's global AppData cache.
    # Reuse the verified distribution already downloaded by cargo inside this
    # workspace, so packaging needs no write access outside the project.
    if (-not $env:CEF_PATH) {
        $cefDirectories = foreach ($profile in @('release', 'debug')) {
            $buildDirectory = Join-Path $env:CARGO_TARGET_DIR "$profile/build"
            if (Test-Path -LiteralPath $buildDirectory -PathType Container) {
                Get-ChildItem -LiteralPath $buildDirectory -Directory -Filter 'cef-dll-sys-*' |
                    ForEach-Object { Join-Path $_.FullName 'out/cef_windows_x86_64' }
            }
        }
        $cef = $cefDirectories | Where-Object {
            (Test-Path -LiteralPath (Join-Path $_ 'archive.json') -PathType Leaf) -and
            (Test-Path -LiteralPath (Join-Path $_ 'libcef.dll') -PathType Leaf)
        } | Select-Object -First 1
        if (-not $cef) { throw 'CEF distribution missing from local cargo build output' }
        $env:CEF_PATH = $cef
    }
    if ($env:TAURI_SIGNING_PRIVATE_KEY) {
        & npm.cmd run tauri -- build --bundles nsis
    } else {
        # The local installer remains unsigned; do not change updater trust in
        # the checked-in manifest or fabricate an updater signature.
        $override = [IO.Path]::GetTempFileName()
        try {
            '{"bundle":{"createUpdaterArtifacts":false}}' | Set-Content -LiteralPath $override -Encoding ascii
            & npm.cmd run tauri -- build --bundles nsis --config $override
        } finally { Remove-Item -LiteralPath $override -Force }
    }
    if ($LASTEXITCODE -ne 0) { throw 'Tauri NSIS build failed' }
    $bundle = Join-Path $env:CARGO_TARGET_DIR 'release/bundle/nsis'
    $source = Get-ChildItem -LiteralPath $bundle -Filter '*-setup.exe' -File |
        Where-Object { $_.Name.Contains($version) -and $_.LastWriteTime -ge $buildStarted.AddSeconds(-5) } |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $source -or $source.Length -lt 1MB) { throw 'No fresh nonempty NSIS installer was produced' }
    if (-not $AcceptanceReport) {
        $candidate = Join-Path $bundle 'Atlas-internal-2.0.1-alpha-windows-x64.exe'
        Move-Item -LiteralPath $source.FullName -Destination $candidate -Force
        Write-Output "Internal build only: $candidate"
        Write-Output "SHA-256: $((Get-FileHash -LiteralPath $candidate -Algorithm SHA256).Hash)"
        return
    }
    $destination = Join-Path $downloads $name
    Copy-Item -LiteralPath $source.FullName -Destination $destination -Force
    $sourceHash = (Get-FileHash -LiteralPath $source.FullName -Algorithm SHA256).Hash
    $copyHash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash
    if ($sourceHash -ne $copyHash) { throw 'Installer SHA-256 differs after copy' }
    $signature = (Get-AuthenticodeSignature -LiteralPath $destination).Status.ToString()
    $report = Join-Path $downloads 'Atlas-Setup-2.0.1-alpha-windows-x64-build-report.txt'
    @(
        "Version: $version"
        "Build-ID: $env:ATLAS_BUILD_ID"
        "Source: $($source.FullName)"
        "Installer: $destination"
        "Bytes: $((Get-Item -LiteralPath $destination).Length)"
        "SHA-256: $copyHash"
        "Authenticode: $signature"
        "Updater key configured: $([bool]$env:TAURI_SIGNING_PRIVATE_KEY)"
        "Tests skipped: $([bool]$SkipTests)"
        "Acceptance report: $AcceptanceReport"
        "Built: $((Get-Date).ToString('o'))"
    ) | Set-Content -LiteralPath $report -Encoding utf8
    $hashFile = Join-Path $downloads 'Atlas-Setup-2.0.1-alpha-windows-x64.sha256'
    "$copyHash  $name" | Set-Content -LiteralPath $hashFile -Encoding ascii
    Write-Output $destination
    Write-Output $report
    Write-Output $hashFile
} finally { Pop-Location }
