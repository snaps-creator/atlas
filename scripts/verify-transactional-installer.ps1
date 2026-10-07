param(
    [Parameter(Mandatory=$true)][string]$Installer,
    [Parameter(Mandatory=$true)][string]$ExpectedPayload,
    [Parameter(Mandatory=$true)][string]$EvidenceDirectory,
    [string]$ExpectedVersion
)
$ErrorActionPreference='Stop'
if (-not $ExpectedVersion) { $ExpectedVersion = (Get-Content (Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version }
$installerPath=(Resolve-Path -LiteralPath $Installer).Path
$source=(Resolve-Path -LiteralPath $ExpectedPayload).Path
New-Item -ItemType Directory -Path $EvidenceDirectory -Force | Out-Null
$evidence=(Resolve-Path -LiteralPath $EvidenceDirectory).Path
$extract=Join-Path $evidence ('extracted-'+[guid]::NewGuid().ToString('N'))
& 'C:/Program Files/7-Zip/7z.exe' x $installerPath "-o$extract" -y *> (Join-Path $evidence 'extract.log')
if ($LASTEXITCODE -ne 0) { throw 'Installer extraction failed' }
$payload=Join-Path $extract '$PLUGINSDIR/payload'
$expected=@(Get-ChildItem -LiteralPath $source -Recurse -File | ForEach-Object { $_.FullName.Substring($source.Length+1) } | Sort-Object)
$actual=@(Get-ChildItem -LiteralPath $payload -Recurse -File | ForEach-Object { $_.FullName.Substring($payload.Length+1) } | Sort-Object)
if (Compare-Object $expected $actual) { throw 'Extracted installer inventory differs' }
$files=@(foreach ($relative in $expected) {
    $from=Join-Path $source $relative
    $to=Join-Path $payload $relative
    $hash=(Get-FileHash -LiteralPath $to -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -ne (Get-FileHash -LiteralPath $from -Algorithm SHA256).Hash.ToLowerInvariant()) { throw "Installer byte mismatch: $relative" }
    [ordered]@{path=$relative.Replace('\','/');sha256=$hash;size=(Get-Item -LiteralPath $to).Length}
})
function Invoke-PayloadVerifier([string]$Label) {
    $process=Start-Process -FilePath (Join-Path $payload 'AtlasUpdater.exe') -ArgumentList ('--verify-local-payload "'+$payload+'"') -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $evidence "$Label.json") -RedirectStandardError (Join-Path $evidence "$Label.err")
    $handle=$process.Handle
    if (-not $process.WaitForExit(30000)) { throw 'Read-only payload verification timed out' }
    $process.WaitForExit()
    return $process.ExitCode
}
if ((Invoke-PayloadVerifier 'payload') -ne 0) { throw 'Embedded receipt rejected extracted installer' }
# Corrupt only an extracted disposable copy. Never alter the build inputs.
$probe=Join-Path $payload 'resources/geoip-manifest.json'
$original=[IO.File]::ReadAllBytes($probe)
try {
    [IO.File]::AppendAllText($probe,' ')
    if ((Invoke-PayloadVerifier 'corrupted-payload') -eq 0) { throw 'Corrupted payload was accepted' }
} finally { [IO.File]::WriteAllBytes($probe,$original) }
if ((Invoke-PayloadVerifier 'restored-payload') -ne 0) { throw 'Restored payload verification failed' }
$metadata=@(foreach ($name in @('Atlas.exe','Atlas.Service.exe','AtlasUpdater.exe','AtlasMaintenance.exe')) {
    $item=Get-Item -LiteralPath (Join-Path $payload $name)
    if ($item.VersionInfo.ProductVersion -ne $ExpectedVersion) { throw "Wrong product version: $name" }
    $stream=[IO.File]::OpenRead($item.FullName)
    $reader=[IO.BinaryReader]::new($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) { throw 'Invalid executable DOS header' }
        $stream.Position=0x3C
        $peOffset=$reader.ReadUInt32()
        $stream.Position=$peOffset
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) { throw "Executable is not Windows x64: $name" }
    } finally { $reader.Dispose(); $stream.Dispose() }
    [ordered]@{name=$name;version=$item.VersionInfo.ProductVersion;architecture='x64'}
})
& (Join-Path $PSScriptRoot 'test-release-payload.ps1') -Payload $payload -Installer $installerPath -ExpectedVersion $ExpectedVersion
$installerFile=Get-Item -LiteralPath $installerPath
if ($installerFile.VersionInfo.ProductVersion -ne $ExpectedVersion) { throw 'Wrong installer product version' }
[ordered]@{version=$ExpectedVersion;installer=$installerPath;sha256=(Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash.ToLowerInvariant();size=$installerFile.Length;payloadVerified=$true;corruptedPayloadRejected=$true;installed=$false;files=$files;metadata=$metadata} |
    ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $evidence 'verification.json') -Encoding utf8
Write-Output "Verified $($files.Count) packaged files, version metadata and corruption rejection. Installer was not run."
