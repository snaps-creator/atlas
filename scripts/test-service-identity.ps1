param([Parameter(Mandatory=$true)][string]$Executable, [switch]$RequirePackagedService)
$ErrorActionPreference='Stop'
$exe=(Resolve-Path -LiteralPath $Executable).Path
$version=(Get-Item -LiteralPath $exe).VersionInfo
if ([version]("{0}.{1}.{2}.{3}" -f $version.FileMajorPart,$version.FileMinorPart,$version.FileBuildPart,$version.FilePrivatePart) -lt [version]'2.2.3.0') {
    throw 'Older binaries do not implement the isolated identity mode; refusing to launch them'
}
$packagedService=Join-Path (Split-Path $exe) 'Atlas.Service.exe'
if ($RequirePackagedService -and -not (Test-Path -LiteralPath $packagedService -PathType Leaf)) { throw 'Required packaged Atlas.Service.exe is missing' }
if (Test-Path -LiteralPath $packagedService) {
    if ((Get-FileHash -LiteralPath $packagedService).Hash -ne (Get-FileHash -LiteralPath $exe).Hash) { throw 'Packaged service differs from application binary' }
}
# Never create or delete a sibling in the input directory. Both old flat
# packages and new transactional packages are tested in a private copy.
$fixture=Join-Path $env:TEMP ('atlas-identity-acceptance-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
Get-ChildItem -LiteralPath (Split-Path $exe) | Copy-Item -Destination $fixture -Recurse
$exe=Join-Path $fixture 'Atlas.exe'
$service=Join-Path $fixture 'Atlas.Service.exe'
if (-not (Test-Path -LiteralPath $service)) { Copy-Item -LiteralPath $exe -Destination $service }
$previous=$env:ATLAS_SERVICE_IDENTITY_SMOKE
$env:ATLAS_SERVICE_IDENTITY_SMOKE='1'
$owner=$null
try {
    $info=[Diagnostics.ProcessStartInfo]::new($exe,'--identity-owner')
    $info.UseShellExecute=$false; $info.CreateNoWindow=$true; $info.RedirectStandardInput=$true
    $info.WorkingDirectory=Split-Path $exe
    $owner=[Diagnostics.Process]::Start($info)
    $check=Start-Process -FilePath $service -ArgumentList @('--identity-check',$owner.Id) -WorkingDirectory (Split-Path $exe) -WindowStyle Hidden -PassThru
    if(-not $check.WaitForExit(10000)){ $check.Kill(); throw 'Service identity check timed out' }
    if($check.ExitCode -ne 0){throw 'Atlas.Service.exe rejected its own Atlas.exe owner'}
    if($owner.HasExited){throw 'Owner fixture exited before identity acceptance'}
    'PASS: packaged Atlas.Service.exe accepts the exact sibling Atlas.exe owner; no service or VPN started.'
} finally {
    if($owner -and -not $owner.HasExited){$owner.StandardInput.Close(); if(-not $owner.WaitForExit(3000)){$owner.Kill();$owner.WaitForExit()}}
    $env:ATLAS_SERVICE_IDENTITY_SMOKE=$previous
    $resolved=[IO.Path]::GetFullPath($fixture)
    $tempRoot=[IO.Path]::GetFullPath($env:TEMP).TrimEnd('\')+'\'
    if (-not $resolved.StartsWith($tempRoot,[StringComparison]::OrdinalIgnoreCase) -or (Split-Path $resolved -Leaf) -notlike 'atlas-identity-acceptance-*') { throw 'Unsafe identity fixture cleanup path' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
