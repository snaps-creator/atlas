$ErrorActionPreference = 'Stop'
# Account creation is confined to disposable CI; never alter workstation users.
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'This test requires a disposable GitHub Windows runner'
}
$name = 'AtlasTest' + [guid]::NewGuid().ToString('N').Substring(0,10)
$password = [guid]::NewGuid().ToString('N') + '!aA9'
$secure = ConvertTo-SecureString $password -AsPlainText -Force
$created = $false
$profileProcess = $null
try {
    New-LocalUser -Name $name -Password $secure -AccountNeverExpires | Out-Null
    $created = $true
    $users = Get-LocalGroup -SID 'S-1-5-32-545'
    Add-LocalGroupMember -Group $users -Member $name
    # Keep the real user's registry hive loaded, as Explorer does during UAC.
    $credential = [pscredential]::new("$env:COMPUTERNAME\$name",$secure)
    $profileProcess = Start-Process -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" `
        -Credential $credential -LoadUserProfile -WindowStyle Hidden -PassThru `
        -WorkingDirectory "$env:SystemRoot\System32" `
        -ArgumentList '-NoProfile -NonInteractive -Command "Start-Sleep -Seconds 300"'
    $env:ATLAS_TEST_DESKTOP_PID = [string]$profileProcess.Id
    cargo test --locked --manifest-path src-tauri/Cargo.toml --bin AtlasUpdater -- --exact update_user::tests::credential_elevation_uses_desktop_account --ignored
    if ($LASTEXITCODE -ne 0) { throw 'Alternate-user installer regression failed' }
    cargo test --locked --manifest-path src-tauri/Cargo.toml --bin AtlasUpdater -- --exact update_user::tests::elevated_shell_is_reduced_before_desktop_launch --ignored
    if ($LASTEXITCODE -ne 0) { throw 'Elevated-shell installer regression failed' }
} finally {
    Remove-Item Env:ATLAS_TEST_DESKTOP_PID -ErrorAction SilentlyContinue
    if ($profileProcess -and -not $profileProcess.HasExited) { $profileProcess.Kill(); $profileProcess.WaitForExit() }
    if ($created) { Remove-LocalUser -Name $name }
}
