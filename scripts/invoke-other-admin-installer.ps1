param([string]$Installer,[string]$InstallRoot,[string]$OutputDirectory,[switch]$Worker,[string]$Ticket)
$ErrorActionPreference='Stop'
if ($Worker) {
    $identity=[Security.Principal.WindowsIdentity]::GetCurrent()
    if ($identity.Name.Split('\')[-1] -notmatch '^AtlasInst[0-9a-f]{10}$') { throw 'Private disposable installer account required' }
    $admin=[Security.Principal.WindowsPrincipal]::new($identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $admin) {
        $hostPath=(Get-Process -Id $PID).Path
        $elevated=Start-Process -FilePath $hostPath -Verb RunAs -WindowStyle Hidden -PassThru `
            -ArgumentList ('-NoProfile -ExecutionPolicy Bypass -File "'+$PSCommandPath+'" -Worker -Ticket "'+$Ticket+'"')
        $handle=$elevated.Handle
        if (-not $elevated.WaitForExit(240000)) { throw 'Alternate administrator elevation timed out' }
        $elevated.WaitForExit()
        exit $elevated.ExitCode
    }
    $config=Get-Content -LiteralPath $Ticket -Raw | ConvertFrom-Json
    if ($identity.User.Value -eq $config.desktopSid -or (Get-Process -Id $PID).SessionId -ne $config.session) {
        throw 'Installer must use a different administrator in the same Windows session'
    }
    $directory=Split-Path $Ticket -Parent
    try {
        $process=Start-Process -FilePath $config.installer -ArgumentList @('/S','/UPDATE',('/D='+$config.installRoot)) -WindowStyle Hidden -PassThru
        $handle=$process.Handle
        if (-not $process.WaitForExit(180000)) { throw 'Alternate administrator installer exceeded three minutes' }
        $process.WaitForExit()
        [ordered]@{exitCode=$process.ExitCode;installerSid=$identity.User.Value;desktopSid=$config.desktopSid;session=$config.session} |
            ConvertTo-Json | Set-Content -LiteralPath (Join-Path $directory 'result.json') -Encoding utf8
    } finally {
        $log=Join-Path $env:TEMP 'atlas-transactional-install.log'
        if (Test-Path -LiteralPath $log) { Copy-Item -LiteralPath $log -Destination (Join-Path $directory 'atlas-transactional-install.log') }
    }
    exit 0
}
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Disposable GitHub Windows runner required' }
$policy=Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System'
if ($policy.EnableLUA -ne 1 -or $policy.ConsentPromptBehaviorAdmin -ne 0) { throw 'Existing prompt-free hosted UAC policy required; this test never changes it' }
$directory=Join-Path $OutputDirectory ('credential-install-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $directory | Out-Null
$identity=[Security.Principal.WindowsIdentity]::GetCurrent()
$ticketPath=Join-Path $directory 'ticket.json'
[ordered]@{installer=(Resolve-Path -LiteralPath $Installer).Path;installRoot=$InstallRoot;desktopSid=$identity.User.Value;session=(Get-Process -Id $PID).SessionId} |
    ConvertTo-Json | Set-Content -LiteralPath $ticketPath -Encoding utf8
$name='AtlasInst'+[guid]::NewGuid().ToString('N').Substring(0,10)
$password=ConvertTo-SecureString ([guid]::NewGuid().ToString('N')+'!aA9') -AsPlainText -Force
$created=$false;$workerProcess=$null
try {
    New-LocalUser -Name $name -Password $password -AccountNeverExpires | Out-Null
    $created=$true
    Add-LocalGroupMember -Group (Get-LocalGroup -SID 'S-1-5-32-544') -Member $name
    $credential=[pscredential]::new("$env:COMPUTERNAME\$name",$password)
    $workerProcess=Start-Process -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" `
        -Credential $credential -LoadUserProfile -WindowStyle Hidden -PassThru -WorkingDirectory "$env:SystemRoot\System32" `
        -ArgumentList ('-NoProfile -ExecutionPolicy Bypass -File "'+$PSCommandPath+'" -Worker -Ticket "'+$ticketPath+'"')
    $handle=$workerProcess.Handle
    if (-not $workerProcess.WaitForExit(270000)) { throw 'Alternate administrator worker timed out' }
    $workerProcess.WaitForExit()
    if ($workerProcess.ExitCode -ne 0) { throw "Alternate administrator worker failed: $($workerProcess.ExitCode)" }
    $result=Get-Content -LiteralPath (Join-Path $directory 'result.json') -Raw | ConvertFrom-Json
    if ($result.exitCode -ne 0) { Get-Content -LiteralPath (Join-Path $directory 'atlas-transactional-install.log') -Tail 40 | Write-Host }
    return $result
} finally {
    if ($workerProcess) { $workerProcess.Dispose() }
    if ($created) { Remove-LocalUser -Name $name }
}
