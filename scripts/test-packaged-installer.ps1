param([Parameter(Mandatory=$true)][string]$Installer)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
& node (Join-Path $PSScriptRoot 'verify-installer.cjs') $Installer
if ($LASTEXITCODE -ne 0) { throw 'Installer signature verification failed' }
$sevenZip = Join-Path $env:ProgramFiles '7-Zip/7z.exe'
if (-not (Test-Path -LiteralPath $sevenZip)) { $sevenZip = (Get-Command 7z.exe -ErrorAction Stop).Source }
$extract = Join-Path $env:TEMP ('atlas-package-acceptance-' + [guid]::NewGuid().ToString('N'))
& $sevenZip x (Resolve-Path -LiteralPath $Installer).Path "-o$extract" -y | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Installer extraction failed' }
$manifest = Get-Content (Join-Path $repo 'src-tauri/resources/xray-manifest.json') -Raw | ConvertFrom-Json
foreach ($entry in @(@('Atlas.Xray.exe','brandedSha256'),@('xray-assets/geoip.dat','geoipSha256'),@('xray-assets/geosite.dat','geositeSha256'))) {
    $path = Join-Path $extract ('resources/' + $entry[0])
    if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $manifest.($entry[1])) { throw "Packaged resource hash differs: $($entry[0])" }
}
& (Join-Path $PSScriptRoot 'test-ui-startup.ps1') -Executable (Join-Path $extract 'Atlas.exe')
& (Join-Path $PSScriptRoot 'test-service-identity.ps1') -Executable (Join-Path $extract 'Atlas.exe')
# Remove only this uniquely created test directory, never an installation.
$resolved = [IO.Path]::GetFullPath($extract)
$expectedRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
if (-not $resolved.StartsWith($expectedRoot, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path $resolved -Leaf) -notlike 'atlas-package-acceptance-*') { throw 'Unsafe test cleanup path' }
Remove-Item -LiteralPath $resolved -Recurse -Force
