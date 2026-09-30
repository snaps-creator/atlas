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
# The repair helper must actually be embedded and start without any CEF DLLs.
# --inspect is read-only: this acceptance never stops the installed VPN/service.
$helper = @(Get-ChildItem -LiteralPath $extract -Filter AtlasMaintenance.exe -File -Recurse)
if ($helper.Count -lt 1 -or $helper.Count -gt 2) { throw 'Installer native maintenance payload is missing or ambiguous' }
$helperHashes = @($helper | ForEach-Object { (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash } | Select-Object -Unique)
if ($helperHashes.Count -ne 1 -or -not ($helper | Where-Object { $_.Directory.Name -eq '$PLUGINSDIR' })) { throw 'Preinstall helper must match the installed maintenance binary' }
$isolated = Join-Path $extract 'native-helper-only'
New-Item -ItemType Directory -Path $isolated | Out-Null
$isolatedExe = Join-Path $isolated 'AtlasMaintenance.exe'
Copy-Item -LiteralPath $helper[0].FullName -Destination $isolatedExe
$output = Join-Path $isolated 'inspection.json'
$errors = Join-Path $isolated 'inspection.err'
$native = Start-Process -FilePath $isolatedExe -ArgumentList '--inspect' -WorkingDirectory $isolated -WindowStyle Hidden -PassThru -RedirectStandardOutput $output -RedirectStandardError $errors
if (-not $native.WaitForExit(10000)) { Stop-Process -Id $native.Id -Force; throw 'Native maintenance helper did not finish' }
if ($native.ExitCode -ne 0) { throw 'Native maintenance helper failed without CEF' }
$inspection = Get-Content -LiteralPath $output -Raw | ConvertFrom-Json
if ($null -eq $inspection.verifiedWintunDriver) { throw 'Native helper did not return driver verification evidence' }
Write-Output 'PASS: embedded native maintenance starts independently of CEF; read-only adapter inspection completed.'
& (Join-Path $PSScriptRoot 'test-ui-startup.ps1') -Executable (Join-Path $extract 'Atlas.exe')
& (Join-Path $PSScriptRoot 'test-service-identity.ps1') -Executable (Join-Path $extract 'Atlas.exe')
# Remove only this uniquely created test directory, never an installation.
$resolved = [IO.Path]::GetFullPath($extract)
$expectedRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
if (-not $resolved.StartsWith($expectedRoot, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path $resolved -Leaf) -notlike 'atlas-package-acceptance-*') { throw 'Unsafe test cleanup path' }
Remove-Item -LiteralPath $resolved -Recurse -Force
