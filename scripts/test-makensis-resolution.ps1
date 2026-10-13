$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'resolve-makensis.ps1')
$previous = $env:MAKENSIS
try {
    # Mock discovery only: no compiler process, registry or installation writes.
    $script:files = @('explicit.exe', 'environment.exe', 'standard/NSIS/makensis.exe')
    function Test-Path { param($LiteralPath, $PathType) $LiteralPath.Replace('\', '/') -in $script:files }
    function Resolve-Path { param($LiteralPath) [pscustomobject]@{Path=$LiteralPath} }
    function Get-Command { param($Name, $ErrorAction) if ($script:pathCompiler) { [pscustomobject]@{Source=$script:pathCompiler} } }
    function Assert-Equal($actual, $expected) { if ($actual -ne $expected) { throw "Expected $expected; got $actual" } }
    $env:MAKENSIS = 'environment.exe'
    $script:pathCompiler = 'path.exe'
    Assert-Equal (Resolve-AtlasMakensis 'explicit.exe') 'explicit.exe'
    Assert-Equal (Resolve-AtlasMakensis) 'environment.exe'
    $env:MAKENSIS = ''
    Assert-Equal (Resolve-AtlasMakensis) 'path.exe'
    $script:pathCompiler = $null
    function Join-Path { param($Path, $ChildPath) "standard/$ChildPath" }
    Assert-Equal ((Resolve-AtlasMakensis).Replace('\','/')) 'standard/NSIS/makensis.exe'
    $script:files = @()
    foreach ($explicit in @('', 'missing.exe')) {
        $failed = $false
        try { Resolve-AtlasMakensis $explicit | Out-Null } catch { $failed = $_.Exception.Message -match 'NSIS' }
        if (-not $failed) { throw 'Missing compiler must produce an actionable error' }
    }
    'PASS: explicit, environment, PATH, standard discovery and failure'
} finally { $env:MAKENSIS = $previous }
