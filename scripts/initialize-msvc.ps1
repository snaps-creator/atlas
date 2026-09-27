$ErrorActionPreference = 'Stop'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$installation = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $installation) { throw 'Visual Studio C++ tools are required' }
$dev = Join-Path $installation 'Common7/Tools/VsDevCmd.bat'
$values = & cmd.exe /d /c "call `"$dev`" -arch=amd64 >nul && set"
if ($LASTEXITCODE -ne 0) { throw 'MSVC environment initialization failed' }
foreach ($line in $values) {
    if ($line -match '^([^=]+)=(.*)$') {
        [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
    }
}
$cmake = Join-Path $installation 'Common7/IDE/CommonExtensions/Microsoft/CMake'
$env:PATH = (Join-Path $cmake 'CMake/bin') + ';' + (Join-Path $cmake 'Ninja') + ';' + $env:PATH
if ($env:GITHUB_ENV) {
    foreach ($name in @('PATH','LIB','LIBPATH','INCLUDE','VCINSTALLDIR','VCToolsInstallDir','WindowsSdkDir','WindowsSDKVersion')) {
        $value = [Environment]::GetEnvironmentVariable($name, 'Process')
        if ($value) { "$name=$value" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append }
    }
}
