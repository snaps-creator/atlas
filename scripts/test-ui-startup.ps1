param([Parameter(Mandatory=$true)][string]$Executable)
$ErrorActionPreference = 'Stop'
$exe = (Resolve-Path -LiteralPath $Executable).Path
$directory = Split-Path $exe -Parent
$evidenceRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }
$evidence = Join-Path $evidenceRoot ('atlas-ui-acceptance-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $evidence | Out-Null
$report = Join-Path $evidence 'report.json'
$previousMode = $env:ATLAS_UI_PROCESS_SMOKE
$previousReport = $env:ATLAS_UI_SMOKE_REPORT
try {
    $env:ATLAS_UI_PROCESS_SMOKE = '1'
    $env:ATLAS_UI_SMOKE_REPORT = $report
    $process = Start-Process -FilePath $exe -WorkingDirectory $directory -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $evidence 'stdout.log') -RedirectStandardError (Join-Path $evidence 'stderr.log')
    $null = $process.Handle
    $observer = Start-Process -FilePath (Get-Command pwsh).Source -ArgumentList @('-NoProfile','-File',('"'+(Join-Path $PSScriptRoot 'observe-ui-waits.ps1')+'"'),'-ObservedPid',$process.Id,'-Executable',('"'+$exe+'"'),'-Evidence',('"'+$evidence+'"')) -WindowStyle Hidden -PassThru
    $exited = $process.WaitForExit(25000)
    $renderAcknowledged = $false
    if (Test-Path -LiteralPath $report) {
        $ack = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
        $renderAcknowledged = $ack.rendered -eq $true -and $ack.buttons -ge 5
    }
    if (-not $exited -and $renderAcknowledged) {
        # Startup already passed within its original deadline. CEF teardown is
        # a separate lifecycle stage, including in immutable historical fixtures.
        # Do not retry or kill-and-pass: require a normal bounded process exit.
        Write-Output 'Render/IPC acknowledged within startup deadline; awaiting normal CEF shutdown.'
        $exited = $process.WaitForExit(25000)
    }
    if (-not $exited) {
        Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $exe } |
            Select-Object ProcessId,ParentProcessId,ExecutablePath,CommandLine | ConvertTo-Json |
            Set-Content (Join-Path $evidence 'processes.json')
        if (Test-Path -LiteralPath $report) { Get-Content -LiteralPath $report }
        if (Test-Path -LiteralPath "$report.lifecycle.log") { Get-Content -LiteralPath "$report.lifecycle.log" }
        Get-Content (Join-Path $evidence 'stderr.log') -Tail 40
        try {
            & (Join-Path $PSScriptRoot 'test-ui-waits.ps1') -ProcessId $process.Id -Executable $exe |
                Set-Content (Join-Path $evidence 'wait-chains.json')
        } catch { Write-Warning "Wait-chain inspection unavailable: $($_.Exception.Message)" }
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        if ($renderAcknowledged) { throw 'Atlas UI rendered but did not exit within the additional 25-second shutdown deadline' }
        throw 'Atlas UI did not acknowledge rendering/IPC within 25 seconds'
    }
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) { throw "Isolated Atlas UI exited with failure code $($process.ExitCode)" }
    if (-not (Test-Path -LiteralPath $report)) { throw 'Atlas created a process but failed to render its interface and acknowledge IPC' }
    $result = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
    if (-not $result.rendered -or $result.buttons -lt 5) { throw 'Atlas UI acceptance report is incomplete' }
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $remaining = @(Get-CimInstance Win32_Process -Filter "Name='Atlas.exe'" | Where-Object { $_.ExecutablePath -eq $exe })
        if ($remaining.Count -eq 0) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($remaining.Count -ne 0) {
        $remaining | Select-Object ProcessId,ParentProcessId,ExecutablePath,CommandLine |
            ConvertTo-Json | Set-Content (Join-Path $evidence 'processes.json')
        Write-Output "UI process evidence: $evidence"
        throw 'Isolated Atlas UI left helper processes behind'
    }
    $result | ConvertTo-Json -Compress
    Write-Output 'PASS: packaged React interface rendered, IPC responded, all isolated UI processes exited; no VPN/service setup executed.'
} finally {
    $env:ATLAS_UI_PROCESS_SMOKE = $previousMode
    $env:ATLAS_UI_SMOKE_REPORT = $previousReport
    # Keep the render acknowledgement and logs for CI failure artifacts.
}

