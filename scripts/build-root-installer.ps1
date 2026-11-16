param([string]$TargetDirectory='temp/root-installer-target')
$ErrorActionPreference='Stop'
$repo=Split-Path $PSScriptRoot -Parent
Set-Location -LiteralPath $repo
. (Join-Path $PSScriptRoot 'initialize-msvc.ps1')
$env:CARGO_TARGET_DIR=[IO.Path]::GetFullPath((Join-Path $repo $TargetDirectory))
$env:CARGO_INCREMENTAL='0'
$previousRustFlags=$env:RUSTFLAGS
try {
    # This helper runs before the full package can provide the VC++ runtime.
    # https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes
    $env:RUSTFLAGS=($previousRustFlags+' -C target-feature=+crt-static').Trim()
    cargo build --locked --offline --release --target x86_64-pc-windows-msvc --manifest-path scripts/root-installer/Cargo.toml --bin AtlasSetupHost
    if ($LASTEXITCODE -ne 0) { throw 'Root installer native verifier build failed' }
} finally { $env:RUSTFLAGS=$previousRustFlags }
$helper=Join-Path $env:CARGO_TARGET_DIR 'x86_64-pc-windows-msvc/release/AtlasSetupHost.exe'
node (Join-Path $PSScriptRoot 'verify-root-helper.cjs') $helper
if ($LASTEXITCODE -ne 0) { throw 'Root helper requires a non-Windows runtime' }
$generated=Join-Path $env:CARGO_TARGET_DIR 'BootstrapPayload.cs'
$hash=(Get-FileHash -LiteralPath $helper -Algorithm SHA256).Hash.ToLowerInvariant()
$version=(Get-Content package.json -Raw | ConvertFrom-Json).version
$assembly='using System.Reflection; [assembly:AssemblyTitle("Atlas Setup")] [assembly:AssemblyProduct("Atlas")] [assembly:AssemblyVersion("'+$version+'.0")] [assembly:AssemblyFileVersion("'+$version+'.0")] [assembly:AssemblyInformationalVersion("'+$version+'")] '
[IO.File]::WriteAllText($generated,($assembly+'internal static class BootstrapPayload { internal const string Sha256 = "'+$hash+'"; }'))
$compiler=Join-Path $env:SystemRoot 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
& $compiler /nologo /target:winexe /platform:x64 /optimize+ /codepage:65001 /r:System.Windows.Forms.dll /r:System.Drawing.dll /r:System.Web.Extensions.dll `
    ('/out:'+(Join-Path $repo 'Install Atlas.exe')) ('/win32icon:'+(Join-Path $repo 'src-tauri/icons/icon.ico')) ('/resource:'+$helper+',AtlasSetupHost.exe') $generated (Join-Path $PSScriptRoot 'root-installer/Program.cs')
if ($LASTEXITCODE -ne 0) { throw 'Root installer window build failed' }
$sources=@('src-tauri/src/updater_main.rs','src-tauri/tauri.conf.json','scripts/root-installer/Program.cs','scripts/root-installer/Cargo.toml','scripts/root-installer/Cargo.lock','scripts/root-installer/build.rs','scripts/build-root-installer.ps1')
foreach ($match in [regex]::Matches([IO.File]::ReadAllText((Join-Path $repo 'src-tauri/src/updater_main.rs')),'(?m)^mod ([a-z_]+);')) {
    $sources+='src-tauri/src/'+$match.Groups[1].Value+'.rs'
}
$files=@($sources | Sort-Object -Unique | ForEach-Object {
    $bytes=[Text.Encoding]::UTF8.GetBytes([IO.File]::ReadAllText((Join-Path $repo $_)).Replace("`r`n","`n"))
    $sha=[Security.Cryptography.SHA256]::Create()
    try { $sourceHash=([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-','').ToLowerInvariant() } finally { $sha.Dispose() }
    [ordered]@{path=$_;sha256=$sourceHash}
})
[ordered]@{version=(Get-Content package.json -Raw | ConvertFrom-Json).version;exeSha256=(Get-FileHash -LiteralPath 'Install Atlas.exe' -Algorithm SHA256).Hash.ToLowerInvariant();helperSha256=$hash;sources=$files} |
    ConvertTo-Json -Depth 5 | Set-Content -LiteralPath scripts/root-installer/build.json -Encoding utf8
& (Join-Path $PSScriptRoot 'test-root-installer.ps1')
