function Resolve-AtlasMakensis {
    param([string]$ExplicitPath)
    foreach ($specified in @($ExplicitPath, $env:MAKENSIS)) {
        if ($specified) {
            if (-not (Test-Path -LiteralPath $specified -PathType Leaf)) { throw "NSIS compiler not found at configured path: $specified" }
            return (Resolve-Path -LiteralPath $specified).Path
        }
    }
    $command = Get-Command makensis.exe -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    foreach ($base in @($env:LOCALAPPDATA, $env:ProgramFiles, ${env:ProgramFiles(x86)})) {
        if (-not $base) { continue }
        foreach ($relative in @('NSIS/makensis.exe', 'tauri/NSIS/makensis.exe')) {
            $candidate = Join-Path $base $relative
            if (Test-Path -LiteralPath $candidate -PathType Leaf) { return (Resolve-Path -LiteralPath $candidate).Path }
        }
    }
    throw 'NSIS makensis.exe was not found. Supply -Makensis, set MAKENSIS, or install NSIS on PATH.'
}
