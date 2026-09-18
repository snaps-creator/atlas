$ErrorActionPreference = 'Stop'
$atlasVersion = 'v1.19.31'
$atlasExpectedHash = '93d14e9a13b49b2f2d256202d02cc8d14a7c4695edf084cae0f941986bc9c218'
$atlasProject = Split-Path $PSScriptRoot -Parent
$atlasTemp = Join-Path $env:TEMP ('atlas-core-' + [guid]::NewGuid())
New-Item -ItemType Directory $atlasTemp | Out-Null
$atlasArchive = Join-Path $atlasTemp 'mihomo.zip'
Invoke-WebRequest "https://github.com/MetaCubeX/mihomo/releases/download/$atlasVersion/mihomo-windows-amd64-compatible-$atlasVersion.zip" -OutFile $atlasArchive
if ((Get-FileHash $atlasArchive -Algorithm SHA256).Hash.ToLower() -ne $atlasExpectedHash) { throw 'Mihomo SHA-256 mismatch' }
Expand-Archive $atlasArchive (Join-Path $atlasTemp 'expanded')
$atlasResource = Join-Path $atlasProject 'src-tauri/resources'
New-Item -ItemType Directory -Force $atlasResource | Out-Null
$atlasExecutables = @(Get-ChildItem (Join-Path $atlasTemp 'expanded') -Filter '*.exe')
if ($atlasExecutables.Count -ne 1) { throw 'Unexpected archive layout' }
Copy-Item -LiteralPath $atlasExecutables[0].FullName -Destination (Join-Path $atlasResource 'mihomo.exe')
Invoke-WebRequest "https://raw.githubusercontent.com/MetaCubeX/mihomo/$atlasVersion/LICENSE" -OutFile (Join-Path $atlasResource 'LICENSE-mihomo')
Write-Output "Verified Mihomo $atlasVersion is ready."
