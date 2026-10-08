param([Parameter(Mandatory=$true)][string]$Installer, [switch]$VerifyOnly)
$ErrorActionPreference = 'Stop'
& node (Join-Path $PSScriptRoot 'check-updater-readiness.cjs')
if ($LASTEXITCODE -ne 0) { throw 'Updater publication policy blocks preparation' }
if (-not $env:GITHUB_REPOSITORY -or -not $env:ATLAS_BUILD_ID) { throw 'GitHub build/repository context is missing' }
if (-not $VerifyOnly -and (-not $env:GITHUB_TOKEN -or $env:GITHUB_ACTIONS -ne 'true' -or $env:GITHUB_REF -ne 'refs/heads/main' -or $env:ATLAS_BUILD_ID -ne $env:GITHUB_SHA)) {
    throw 'Publication requires the verified main CI build and GitHub authorization'
}
& (Join-Path $PSScriptRoot 'test-packaged-installer.ps1') -Installer $Installer
& node (Join-Path $PSScriptRoot 'prepare-release.cjs') $Installer
if ($LASTEXITCODE -ne 0) { throw 'Signed release metadata preparation failed' }
$directory = Split-Path $Installer -Parent
$prepared = Get-Content -LiteralPath (Join-Path $directory 'release-preparation.json') -Raw | ConvertFrom-Json
$manifestPath = Join-Path $directory 'latest.json'
$signedManifest = Join-Path $directory 'update-manifest.json'
if ($VerifyOnly) {
    Write-Output 'PASS: complete signed publication preparation; no GitHub release or upload performed.'
    return
}
# PR dry-run and actual publication use identical validation and metadata above.
# GitHub mutations are confined to this branch, after all preparation succeeds.
$tag = $prepared.tag
$manifest = $prepared.latest
$base = "https://api.github.com/repos/$env:GITHUB_REPOSITORY"
$headers = @{ Authorization = "Bearer $env:GITHUB_TOKEN"; Accept = 'application/vnd.github+json'; 'User-Agent' = 'Atlas-release' }
$body = @{tag_name=$tag;target_commitish=$env:GITHUB_SHA;name="Atlas $($manifest.version)";body=$manifest.notes;draft=$true;prerelease=$false} | ConvertTo-Json
$release = Invoke-RestMethod -Method Post "$base/releases" -Headers $headers -ContentType 'application/json; charset=utf-8' -Body ([Text.Encoding]::UTF8.GetBytes($body))
foreach ($file in @($Installer,"$Installer.sig",$manifestPath,$signedManifest,"$signedManifest.sig")) {
    $assetName = [Uri]::EscapeDataString((Split-Path $file -Leaf))
    $url = $release.upload_url.Split('{')[0] + '?name=' + $assetName
    Invoke-RestMethod -Method Post $url -Headers $headers -ContentType 'application/octet-stream' -InFile $file | Out-Null
}
Invoke-RestMethod -Method Patch "$base/releases/$($release.id)" -Headers $headers -ContentType 'application/json' -Body '{"draft":false,"make_latest":"true"}' | Select-Object html_url,tag_name
