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
if ((Get-Item -LiteralPath $exe).Length -ge 25MB) { throw 'Root installer exceeds the direct-download size budget' }
if ((Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant() -ne $record.exeSha256) { throw 'Root installer checksum differs' }
$process=Start-Process -FilePath $exe -ArgumentList '--verify' -WindowStyle Hidden -PassThru
$handle=$process.Handle
if (-not $process.WaitForExit(30000)) { throw 'Root installer verification timed out' }
$process.WaitForExit()
if ($process.ExitCode -ne 0) { throw 'Root installer rejected its embedded verifier' }
Write-Output 'PASS: root EXE, version, source inventory and embedded verifier; no installation or download performed.'
