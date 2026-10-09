param([int]$ObservedPid,[string]$Executable,[string]$Evidence)
$ErrorActionPreference='Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'Disposable runner only' }
for ($sample=0; $sample -lt 10; $sample++) {
    Start-Sleep -Seconds 3
    if (-not (Get-Process -Id $ObservedPid -ErrorAction SilentlyContinue)) { break }
    try {
        & (Join-Path $PSScriptRoot 'test-ui-waits.ps1') -ProcessId $ObservedPid -Executable $Executable |
            Set-Content (Join-Path $Evidence "wait-$sample.json")
    } catch { $_.Exception.Message | Set-Content (Join-Path $Evidence "wait-$sample.error") }
}
