param([Parameter(Mandatory=$true)][string]$Executable)
$ErrorActionPreference='Stop'
$exe=(Resolve-Path -LiteralPath $Executable).Path
$version=(Get-Item -LiteralPath $exe).VersionInfo
if ([version]("{0}.{1}.{2}.{3}" -f $version.FileMajorPart,$version.FileMinorPart,$version.FileBuildPart,$version.FilePrivatePart) -lt [version]'2.2.3.0') {
    throw 'Older binaries do not implement the isolated identity mode; refusing to launch them'
}
$service=Join-Path (Split-Path $exe) 'Atlas.Service.exe'
if(Test-Path -LiteralPath $service){throw 'Identity acceptance requires an isolated extracted package, not an installation'}
Copy-Item -LiteralPath $exe -Destination $service
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
    Remove-Item -LiteralPath $service -ErrorAction SilentlyContinue
}
