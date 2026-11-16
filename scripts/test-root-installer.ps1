$ErrorActionPreference='Stop'
$repo=Split-Path $PSScriptRoot -Parent
$record=Get-Content (Join-Path $PSScriptRoot 'root-installer/build.json') -Raw | ConvertFrom-Json
$version=(Get-Content (Join-Path $repo 'package.json') -Raw | ConvertFrom-Json).version
if ($record.version -ne $version) { throw 'Root installer version differs from the release' }
foreach ($source in $record.sources) {
    $bytes=[Text.Encoding]::UTF8.GetBytes([IO.File]::ReadAllText((Join-Path $repo $source.path)).Replace("`r`n","`n"))
    $sha=[Security.Cryptography.SHA256]::Create()
    try { $sourceHash=([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-','').ToLowerInvariant() } finally { $sha.Dispose() }
    if ($sourceHash -ne $source.sha256) {
        throw "Rebuild Install Atlas.exe after changing $($source.path)"
    }
}
$exe=Join-Path $repo 'Install Atlas.exe'
if ((Get-Item -LiteralPath $exe).VersionInfo.ProductVersion -ne $version) { throw 'Root EXE product version differs from the release' }
if ((Get-Item -LiteralPath $exe).Length -ge 25MB) { throw 'Root installer exceeds the direct-download size budget' }
if ((Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant() -ne $record.exeSha256) { throw 'Root installer checksum differs' }
# Inspect the actual embedded helper, not a build-directory copy or metadata.
$inspection=Join-Path $env:TEMP ('atlas-root-inspection-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $inspection | Out-Null
try {
    $assembly=[Reflection.Assembly]::Load([IO.File]::ReadAllBytes($exe))
    $resource=$assembly.GetManifestResourceStream('AtlasSetupHost.exe')
    if (-not $resource) { throw 'Embedded helper is missing' }
    $embedded=Join-Path $inspection 'AtlasSetupHost.exe'
    $output=[IO.File]::Create($embedded)
    try { $resource.CopyTo($output) } finally { $output.Dispose(); $resource.Dispose() }
    if ((Get-FileHash -LiteralPath $embedded -Algorithm SHA256).Hash.ToLowerInvariant() -ne $record.helperSha256) { throw 'Embedded helper differs from its recorded build' }
    node (Join-Path $PSScriptRoot 'verify-root-helper.cjs') $embedded
    if ($LASTEXITCODE -ne 0) { throw 'Embedded root helper requires an external runtime' }
    $protocolOutput=Join-Path $inspection 'protocol.json'
    $native=Start-Process -FilePath $embedded -ArgumentList '--protocol' -WorkingDirectory $inspection -WindowStyle Hidden -PassThru -RedirectStandardOutput $protocolOutput
    $handle=$native.Handle
    if (-not $native.WaitForExit(30000)) { throw 'Embedded helper protocol check timed out' }
    $native.WaitForExit()
    if ($native.ExitCode -ne 0 -or (Get-Content -LiteralPath $protocolOutput -Raw | ConvertFrom-Json).protocol -ne 1) { throw 'Embedded helper cannot run independently' }
} finally {
    $resolved=[IO.Path]::GetFullPath($inspection)
    $tempRoot=[IO.Path]::GetFullPath($env:TEMP).TrimEnd('\')+'\'
    if (-not $resolved.StartsWith($tempRoot,[StringComparison]::OrdinalIgnoreCase) -or (Split-Path $resolved -Leaf) -notlike 'atlas-root-inspection-*') { throw 'Unsafe root inspection cleanup path' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
$process=Start-Process -FilePath $exe -ArgumentList '--verify' -WindowStyle Hidden -PassThru
$handle=$process.Handle
if (-not $process.WaitForExit(30000)) { throw 'Root installer verification timed out' }
$process.WaitForExit()
if ($process.ExitCode -ne 0) { throw 'Root installer rejected its embedded verifier' }
Write-Output 'PASS: root EXE, source inventory, embedded verifier, Windows-only native imports and actual standalone helper launch; no installation or download performed.'
