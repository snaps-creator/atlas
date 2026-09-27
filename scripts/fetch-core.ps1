param([string]$Source = 'C:\Program Files\Clash Verge\verge-mihomo.exe')
$ErrorActionPreference = 'Stop'
$expectedVersion = 'v1.19.29'
$expectedHash = '98986B574E41F92B22ED65AA42A61AD8CADF886CC7B3F76B722CD73A3A52D878'
$destination = Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri\resources\Atlas.Core.exe'
if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) { throw "Reference Mihomo not found: $Source" }
if ((Get-FileHash -LiteralPath $Source -Algorithm SHA256).Hash -ne $expectedHash) {
    throw 'Reference Mihomo SHA-256 mismatch; inspect the new version before changing the pin'
}
$versionOutput = (& $Source -v | Out-String)
if ($LASTEXITCODE -ne 0 -or $versionOutput -notmatch "Mihomo Meta $expectedVersion windows amd64") {
    throw "Reference Mihomo version differs from $expectedVersion"
}
Copy-Item -LiteralPath $Source -Destination $destination -Force
& (Join-Path $PSScriptRoot 'brand-core.ps1') -Executable $destination
if ($LASTEXITCODE -ne 0) { throw 'Core version resource failed' }
$brandedVersion = (& $destination -v | Out-String)
if ($LASTEXITCODE -ne 0 -or $brandedVersion -notmatch "Mihomo Meta $expectedVersion windows amd64") {
    throw 'Branding damaged the Mihomo executable'
}
Write-Output "Atlas.Core.exe contains verified Mihomo $expectedVersion."
