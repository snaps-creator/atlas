param(
    [Parameter(Mandatory=$true)][ValidateSet('Setup','Verify','Cleanup')][string]$Mode,
    [Parameter(Mandatory=$true)][string]$Fixture,
    [Parameter(Mandatory=$true)][string]$InstallRoot
)
$ErrorActionPreference='Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {throw 'Disposable GitHub Windows runner required'}
$allowed=[IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')+'\'
$folder=[IO.Path]::GetFullPath($Fixture)
$installation=[IO.Path]::GetFullPath($InstallRoot)
if (-not $folder.StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase) -or
    -not $installation.StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase) -or
    (Split-Path $folder -Leaf) -notlike 'offline-startup-*') {throw 'Offline hive fixture must stay under RUNNER_TEMP'}
$stateFile=Join-Path $folder 'fixture.json'
$run='Software\Microsoft\Windows\CurrentVersion\Run'
$approved='Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'
function Invoke-FixtureReg([string[]]$Arguments) {
    & reg.exe @Arguments | Out-Null
    if ($LASTEXITCODE -ne 0) {throw 'Disposable offline registry operation failed'}
}
if ($Mode -eq 'Setup') {
    if (Test-Path -LiteralPath $folder) {throw 'Refusing to overwrite an offline hive fixture'}
    New-Item -ItemType Directory -Path $folder | Out-Null
    $nonce=[guid]::NewGuid().ToString('N')
    $sid='S-1-5-21-'+[Convert]::ToUInt32($nonce.Substring(0,8),16)+'-'+[Convert]::ToUInt32($nonce.Substring(8,8),16)+'-'+[Convert]::ToUInt32($nonce.Substring(16,8),16)+'-3999'
    $source='Software\AtlasAcceptance\Offline-'+$nonce
    $profile='SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\'+$sid
    $hive=Join-Path $folder 'NTUSER.DAT'
    $state=[ordered]@{nonce=$nonce;sid=$sid;profile=$profile;source=$source;hive=$hive;mount='AtlasOfflineFixture-'+$nonce}
    $state | ConvertTo-Json | Set-Content -LiteralPath $stateFile -Encoding utf8
    $key=[Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($source)
    try {
        foreach($path in @($run,$approved)) {
            $child=$key.CreateSubKey($path)
            try {
                if ($path -eq $run) {
                    $child.SetValue('Atlas',('"'+(Join-Path $installation 'AtlasUpdater.exe')+'" --launch --autostart'))
                    $child.SetValue('Unrelated','unrelated-fixture-command')
                } else {
                    $child.SetValue('Atlas',[byte[]](2,0,0,0,0,0,0,0,0,0,0,0),[Microsoft.Win32.RegistryValueKind]::Binary)
                    $child.SetValue('Unrelated',[byte[]](3,0,0,0),[Microsoft.Win32.RegistryValueKind]::Binary)
                }
            } finally {$child.Dispose()}
        }
    } finally {$key.Dispose()}
    Invoke-FixtureReg -Arguments @('save',('HKCU\'+$source),$hive,'/y')
    [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($source)
    $existing=[Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($profile)
    if ($existing) {$existing.Dispose();throw 'Synthetic profile SID collision'}
    $registered=[Microsoft.Win32.Registry]::LocalMachine.CreateSubKey($profile)
    try {$registered.SetValue('ProfileImagePath',$folder);$registered.SetValue('AtlasAcceptance',$nonce)}
    finally {$registered.Dispose()}
    Write-Output 'PASS: registered a synthetic offline profile with a real NTUSER.DAT and unrelated startup values.'
    return
}
$state=Get-Content -LiteralPath $stateFile -Raw | ConvertFrom-Json
if ($state.nonce -notmatch '^[a-f0-9]{32}$' -or $state.profile -ne ('SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\'+$state.sid) -or
    $state.source -ne ('Software\AtlasAcceptance\Offline-'+$state.nonce) -or $state.hive -ne (Join-Path $folder 'NTUSER.DAT') -or
    $state.mount -ne ('AtlasOfflineFixture-'+$state.nonce)) {throw 'Invalid offline fixture ownership'}
$expectedSid='S-1-5-21-'+[Convert]::ToUInt32($state.nonce.Substring(0,8),16)+'-'+[Convert]::ToUInt32($state.nonce.Substring(8,8),16)+'-'+[Convert]::ToUInt32($state.nonce.Substring(16,8),16)+'-3999'
if ($state.sid -ne $expectedSid) {throw 'Invalid synthetic profile SID'}
$registered=[Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($state.profile)
try {if ($registered -and ($registered.GetValue('AtlasAcceptance') -ne $state.nonce -or $registered.GetValue('ProfileImagePath') -ne $folder)) {throw 'Synthetic profile ownership changed'}}
finally {if($registered){$registered.Dispose()}}
if ($Mode -eq 'Verify') {
    $existing=[Microsoft.Win32.Registry]::Users.OpenSubKey($state.mount)
    if ($existing) {$existing.Dispose();throw 'Refusing to replace a loaded registry fixture'}
    Invoke-FixtureReg -Arguments @('load',('HKU\'+$state.mount),$state.hive)
    try {
        foreach($path in @($run,$approved)) {
            $child=[Microsoft.Win32.Registry]::Users.OpenSubKey($state.mount+'\'+$path)
            try {
                if (-not $child -or $child.GetValueNames() -contains 'Atlas') {throw 'Uninstall left an offline Atlas startup registration'}
                if ($child.GetValueNames() -notcontains 'Unrelated') {throw 'Uninstall removed an unrelated offline startup registration'}
                if ($path -eq $run -and $child.GetValue('Unrelated') -ne 'unrelated-fixture-command') {throw 'Unrelated command changed'}
                if ($path -eq $approved -and [Convert]::ToHexString([byte[]]$child.GetValue('Unrelated')) -ne '03000000') {throw 'Unrelated approval changed'}
            } finally {if($child){$child.Dispose()}}
        }
    } finally {Invoke-FixtureReg -Arguments @('unload',('HKU\'+$state.mount))}
    Write-Output 'PASS: real offline hive has no Atlas Run/StartupApproved values; unrelated values survived.'
} else {
    if ($registered) {[Microsoft.Win32.Registry]::LocalMachine.DeleteSubKeyTree($state.profile)}
    $source=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($state.source)
    if ($source) {$source.Dispose();[Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($state.source)}
    # Preserve the file/evidence; remove only registrations created by this test.
}
