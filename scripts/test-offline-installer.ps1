$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
function Verify-Exit([string]$exe,[int]$expected) {
    $p = Start-Process -FilePath $exe -ArgumentList '--verify' -WindowStyle Hidden -PassThru -Wait
    if ($p.ExitCode -ne $expected) { throw "Unexpected verification result: $($p.ExitCode), expected $expected" }
}
Verify-Exit (Join-Path $repo 'Install Atlas.exe') 0
$fixture = Join-Path $repo ('tmp/launcher-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path (Join-Path $fixture 'installer') -Force | Out-Null
Copy-Item (Join-Path $repo 'Install Atlas.exe') $fixture
Verify-Exit (Join-Path $fixture 'Install Atlas.exe') 1
$manifest = Get-Content (Join-Path $repo 'installer/manifest.json') -Raw | ConvertFrom-Json
1..$manifest.parts | ForEach-Object { [IO.File]::WriteAllBytes((Join-Path $fixture "installer/atlas-setup.part$_"),[byte[]](1,2,3)) }
Verify-Exit (Join-Path $fixture 'Install Atlas.exe') 1
# A full-length corrupted input must also be rejected by the hash gate.
1..$manifest.parts | ForEach-Object {
    $name = "atlas-setup.part$_"
    Copy-Item (Join-Path $repo "installer/$name") (Join-Path $fixture "installer/$name") -Force
}
$stream = [IO.File]::OpenWrite((Join-Path $fixture 'installer/atlas-setup.part1'))
try { $stream.WriteByte(0) } finally { $stream.Dispose() }
Verify-Exit (Join-Path $fixture 'Install Atlas.exe') 1
Write-Output 'PASS: intact payload accepted; missing, truncated and corrupted payloads rejected; no installation launched.'
