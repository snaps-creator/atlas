$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$version = (Get-Content (Join-Path $repo 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version
$label = if ($version -match '^(\d+\.\d+\.\d+)-alpha(?:\..*)?$') { "Alpha $($Matches[1])" } else { $version }
$name = "Atlas $label Setup.exe"
$installer = Join-Path $repo $name
& node (Join-Path $PSScriptRoot 'verify-installer.cjs') $installer
if ($LASTEXITCODE -ne 0) { throw 'Installer verification failed; refusing to package.' }
$output = Join-Path $repo 'dist-installer'
New-Item -ItemType Directory -Path $output -Force | Out-Null
$hash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
"$hash  $name" | Set-Content (Join-Path $output 'SHA256SUMS.txt') -Encoding ascii
Compress-Archive -LiteralPath $installer,"$installer.sig",(Join-Path $output 'SHA256SUMS.txt') -DestinationPath (Join-Path $output "Atlas-$version-Windows-x64.zip") -CompressionLevel NoCompression -Force
Write-Output (Join-Path $output "Atlas-$version-Windows-x64.zip")
