param([Parameter(Mandatory=$true)][string]$Installer)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$source = (Resolve-Path -LiteralPath $Installer).Path
Push-Location $repo
try {
    node scripts/verify-installer.cjs $source
    if ($LASTEXITCODE -ne 0) { throw 'Source installer signature is invalid' }
    $size = (Get-Item -LiteralPath $source).Length
    $hash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
    $folder = Join-Path $repo 'installer'
    New-Item -ItemType Directory -Force $folder | Out-Null
    $input = [IO.File]::OpenRead($source)
    $parts = 0
    try {
        $buffer = New-Object byte[] (80MB)
        while ($input.Position -lt $input.Length) {
            $count = 0
            while ($count -lt $buffer.Length -and $input.Position -lt $input.Length) {
                $count += $input.Read($buffer, $count, $buffer.Length - $count)
            }
            $parts++
            $output = [IO.File]::Create((Join-Path $folder "atlas-setup.part$parts"))
            try { $output.Write($buffer,0,$count) } finally { $output.Dispose() }
        }
    } finally { $input.Dispose() }
    Copy-Item -LiteralPath "$source.sig" -Destination (Join-Path $folder 'atlas-setup.exe.sig') -Force
    @{sha256=$hash;size=$size;parts=$parts} | ConvertTo-Json | Set-Content (Join-Path $folder 'manifest.json') -Encoding ascii
    $constants = Join-Path $folder 'Payload.cs'
    "internal static class Payload { internal const long Size = ${size}L; internal const int Parts = $parts; internal const string Sha256 = `"$hash`"; }" | Set-Content $constants -Encoding ascii
    $compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
    & $compiler /nologo /codepage:65001 /target:winexe /platform:x64 /optimize+ /reference:System.Windows.Forms.dll /reference:System.Drawing.dll /win32icon:src-tauri\icons\icon.ico "/out:Install Atlas.exe" scripts\installer-launcher\Program.cs installer\Payload.cs
    if ($LASTEXITCODE -ne 0) { throw 'Launcher compilation failed' }
} finally { Pop-Location }



