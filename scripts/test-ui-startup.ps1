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
    if (-not $process.WaitForExit(25000)) {
        Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $exe } |
            Select-Object ProcessId,ParentProcessId,ExecutablePath,CommandLine | ConvertTo-Json |
            Set-Content (Join-Path $evidence 'processes.json')
        if (Test-Path -LiteralPath $report) { Get-Content -LiteralPath $report }
        Get-Content (Join-Path $evidence 'stderr.log') -Tail 40
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        throw 'Atlas UI did not finish isolated startup acceptance within 25 seconds'
    }
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
