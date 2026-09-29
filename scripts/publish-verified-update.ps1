param([Parameter(Mandatory=$true)][string]$Installer)
$ErrorActionPreference = 'Stop'
if (-not $env:GITHUB_TOKEN -or -not $env:GITHUB_REPOSITORY -or -not $env:GITHUB_SHA) { throw 'GitHub publication context is missing' }
& (Join-Path $PSScriptRoot 'test-packaged-installer.ps1') -Installer $Installer
$version = (Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json).version
$name = Split-Path $Installer -Leaf
$tag = "v$version"
$base = "https://api.github.com/repos/$env:GITHUB_REPOSITORY"
$headers = @{ Authorization = "Bearer $env:GITHUB_TOKEN"; Accept = 'application/vnd.github+json'; 'User-Agent' = 'Atlas-release' }
$manifest = @{
    version = $version
    notes = 'Исправления запуска интерфейса, обновления и завершения Atlas. Подробности — в docs/UPDATES.md.'
    pub_date = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
    platforms = @{ 'windows-x86_64' = @{
        signature = (Get-Content -LiteralPath "$Installer.sig" -Raw).Trim()
        url = "https://github.com/$env:GITHUB_REPOSITORY/releases/download/$tag/$name"
    }}
}
$manifestPath = Join-Path (Split-Path $Installer -Parent) 'latest.json'
$manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath -Encoding utf8NoBOM
# Keep the previous channel live until all three assets are uploaded.
$body = @{tag_name=$tag;target_commitish=$env:GITHUB_SHA;name="Atlas $version";body=$manifest.notes;draft=$true;prerelease=$false} | ConvertTo-Json
$release = Invoke-RestMethod -Method Post "$base/releases" -Headers $headers -ContentType 'application/json; charset=utf-8' -Body ([Text.Encoding]::UTF8.GetBytes($body))
foreach ($file in @($Installer,"$Installer.sig",$manifestPath)) {
    $assetName = [Uri]::EscapeDataString((Split-Path $file -Leaf))
    $url = $release.upload_url.Split('{')[0] + '?name=' + $assetName
    Invoke-RestMethod -Method Post $url -Headers $headers -ContentType 'application/octet-stream' -InFile $file | Out-Null
}
Invoke-RestMethod -Method Patch "$base/releases/$($release.id)" -Headers $headers -ContentType 'application/json' -Body '{"draft":false,"make_latest":"true"}' | Select-Object html_url,tag_name
