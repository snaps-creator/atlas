param([string]$Executable=(Join-Path $PSScriptRoot '../src-tauri/target/debug/atlas-vpn.exe'))
$ErrorActionPreference='Stop'
$taskRepo=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$taskExe=(Resolve-Path -LiteralPath $Executable).Path
if(-not $taskExe.StartsWith($taskRepo+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) {
    throw 'Lifecycle fixture must use a workspace binary, never installed Atlas'
}
$taskDirectory=Join-Path $taskRepo ('temp/health-windows-tests/'+[guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $taskDirectory | Out-Null
$taskPrevious=$env:ATLAS_LIFECYCLE_SMOKE_DIR
try {
    $env:ATLAS_LIFECYCLE_SMOKE_DIR=$taskDirectory
    $taskProcess=Start-Process -FilePath $taskExe -WorkingDirectory (Split-Path $taskExe -Parent) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $taskDirectory 'stdout.log') -RedirectStandardError (Join-Path $taskDirectory 'stderr.log')
    $taskHandle=$taskProcess.Handle
    if(-not $taskProcess.WaitForExit(55000)) {
        Stop-Process -Id $taskProcess.Id -Force
        throw "Lifecycle fixture exceeded deadline: $taskDirectory"
    }
    $taskProcess.WaitForExit()
    if($taskProcess.ExitCode -ne 0){throw "Lifecycle fixture failed ($($taskProcess.ExitCode)): $taskDirectory"}
    $taskReport=Get-Content -LiteralPath (Join-Path $taskDirectory 'report.json') -Raw | ConvertFrom-Json
    if(-not $taskReport.passed -or -not $taskReport.oneWindow -or $taskReport.productionSetupExecuted){throw 'Invalid fixture acknowledgement'}
    # Exact binary path only. Never terminate a name-matched production process.
    $taskRemaining=@(Get-CimInstance Win32_Process | Where-Object {$_.ExecutablePath -eq $taskExe})
    if($taskRemaining.Count -ne 0){throw "Fixture left processes behind: $taskDirectory"}
    [pscustomobject]@{Evidence=$taskDirectory;Report=$taskReport} | ConvertTo-Json -Depth 6
} finally {$env:ATLAS_LIFECYCLE_SMOKE_DIR=$taskPrevious}
