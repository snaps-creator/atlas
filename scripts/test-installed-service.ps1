param([string]$Executable = 'C:\Program Files\Atlas\atlas-vpn.exe', [int]$Cycles = 5)
$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this test as a normal, non-elevated user: administrator permissions hide IPC permission bugs.'
}
if ($Cycles -lt 1 -or $Cycles -gt 20) { throw 'Cycles must be between 1 and 20.' }
$binary = (Resolve-Path -LiteralPath $Executable).Path
$service = Get-Service -Name AtlasNetworkService
if ($service.Status -ne 'Stopped') { throw 'Atlas service must be idle before the test.' }
$results = @()
for ($cycle = 1; $cycle -le $Cycles; $cycle++) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process -FilePath $binary -ArgumentList '--check-network-service' -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit(25000)) {
        $process.Kill()
        throw "Service handshake timed out in cycle $cycle"
    }
    if ($process.ExitCode -ne 0) { throw "Service handshake failed in cycle $cycle with exit $($process.ExitCode)" }
    # Do not wait between sessions: exercise the service-stop / next-start race.
    $results += [pscustomobject]@{Cycle=$cycle;ExitCode=$process.ExitCode;Milliseconds=$timer.ElapsedMilliseconds}
}
$deadline = [DateTime]::UtcNow.AddSeconds(10)
do {
    $service.Refresh()
    if ($service.Status -eq 'Stopped') { break }
    Start-Sleep -Milliseconds 100
} while ([DateTime]::UtcNow -lt $deadline)
if ($service.Status -ne 'Stopped') { throw 'Service remained running after the final client exited.' }
[pscustomobject]@{Version=(Get-Item -LiteralPath $binary).VersionInfo.ProductVersion;Elevated=$false;Cycles=$results;FinalServiceState=$service.Status.ToString()} | ConvertTo-Json -Depth 4
