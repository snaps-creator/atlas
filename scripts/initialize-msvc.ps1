$ErrorActionPreference = 'Stop'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$installation = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $installation) { throw 'Visual Studio C++ tools are required' }
$dev = Join-Path $installation 'Common7/Tools/VsDevCmd.bat'
# Build wrappers may call this repeatedly in one process or in later CI steps.
# Re-running VsDevCmd duplicates PATH/INCLUDE/LIB and can exceed cmd's 8191-char
# expansion limit. Reuse only an already initialized x64 compiler environment.
$compiler=Get-Command cl.exe -ErrorAction SilentlyContinue
$initialized=$env:VSCMD_VER -and $env:VSCMD_ARG_TGT_ARCH -eq 'x64' -and $env:LIB -and $env:INCLUDE -and $compiler -and $compiler.Source.StartsWith($installation,[StringComparison]::OrdinalIgnoreCase)
if (-not $initialized) {
    $values = & cmd.exe /d /c "call `"$dev`" -arch=amd64 >nul && set"
    if ($LASTEXITCODE -ne 0) { throw 'MSVC environment initialization failed' }
    foreach ($line in $values) {
        if ($line -match '^([^=]+)=(.*)$') {
            [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
        }
    }
}
$cmake = Join-Path $installation 'Common7/IDE/CommonExtensions/Microsoft/CMake'
$pathParts=@((Join-Path $cmake 'CMake/bin'),(Join-Path $cmake 'Ninja')) + @($env:PATH -split ';')
$env:PATH=(@($pathParts|Where-Object {$_}|Select-Object -Unique) -join ';')
if ($env:GITHUB_ENV) {
    foreach ($name in @('PATH','LIB','LIBPATH','INCLUDE','VCINSTALLDIR','VCToolsInstallDir','WindowsSdkDir','WindowsSDKVersion','VSCMD_VER','VSCMD_ARG_TGT_ARCH')) {
        $value = [Environment]::GetEnvironmentVariable($name, 'Process')
        if ($value) { "$name=$value" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append }
    }
}
