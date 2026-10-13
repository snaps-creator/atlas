param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [string]$OutputDirectory = 'temp/recovery-load'
)
$ErrorActionPreference = 'Stop'
$atlasTestExe = (Resolve-Path -LiteralPath $Executable).Path
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$atlasOutput = (Resolve-Path -LiteralPath $OutputDirectory).Path
$atlasTimer = [Diagnostics.Stopwatch]::StartNew()
$atlasLoadProcess = Start-Process -FilePath $atlasTestExe -WindowStyle Hidden -PassThru `
    -ArgumentList @('scheduler_load_profile','--ignored','--nocapture') `
    -RedirectStandardOutput (Join-Path $atlasOutput 'stdout.log') `
    -RedirectStandardError (Join-Path $atlasOutput 'stderr.log')
$atlasPeak = 0L
$atlasCpu = 0.0
while (-not $atlasLoadProcess.HasExited) {
    $atlasLoadProcess.Refresh()
    $atlasPeak = [Math]::Max($atlasPeak, $atlasLoadProcess.PeakWorkingSet64)
    $atlasCpu = [Math]::Max($atlasCpu, $atlasLoadProcess.TotalProcessorTime.TotalMilliseconds)
    Start-Sleep -Milliseconds 100
}
$atlasLoadProcess.WaitForExit()
$atlasTimer.Stop()
if ($atlasLoadProcess.ExitCode -ne 0) { throw "Load test failed: $($atlasLoadProcess.ExitCode)" }
$atlasRows = @(Get-Content -LiteralPath (Join-Path $atlasOutput 'stdout.log') | Where-Object { $_.StartsWith('LOAD ') } | ForEach-Object { $_.Substring(5) | ConvertFrom-Json })
if ($atlasRows.Count -ne 10) { throw 'Incomplete load evidence' }
$atlasEvidence = @{
    workload = 'Scheduler model: 20% failures; waits scaled to 1/3/20 ms. Not Internet VPN timings.'
    elapsedMs = $atlasTimer.ElapsedMilliseconds
    cpuMs = $atlasCpu
    peakWorkingSetBytes = $atlasPeak
    rows = $atlasRows
}
$atlasEvidence | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $atlasOutput 'results.json') -Encoding utf8
$atlasEvidence | Select-Object workload,elapsedMs,cpuMs,peakWorkingSetBytes | ConvertTo-Json -Compress
