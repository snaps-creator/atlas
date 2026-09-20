param([Parameter(Mandatory=$true)][string]$OutputDirectory, [switch]$AsSystem)
$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not ([Security.Principal.WindowsPrincipal]$identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'This isolated adapter test requires administrator rights.'
}
if ($AsSystem) {
    $taskName = 'Atlas-Adapter-Probe-' + [guid]::NewGuid().ToString('N')
    $arguments = '-NoProfile -ExecutionPolicy Bypass -File "' + $PSCommandPath + '" -OutputDirectory "' + $OutputDirectory + '"'
    $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $arguments
    $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    try {
        Register-ScheduledTask -TaskName $taskName -Action $action -Principal $principal | Out-Null
        Start-ScheduledTask -TaskName $taskName
        Start-Sleep -Seconds 2
        $deadline = (Get-Date).AddSeconds(90)
        while ((Get-ScheduledTask -TaskName $taskName).State -eq 'Running' -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 1 }
    } finally {
        Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    }
    return
}
$binary = Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri\resources\mihomo.exe'
if (Get-NetAdapter -Name 'Atlas-TUN' -IncludeHidden -ErrorAction SilentlyContinue) {
    throw 'Atlas-TUN already exists; refusing to touch an existing adapter.'
}
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$routesBefore = @(Get-NetRoute | Where-Object { $_.DestinationPrefix -in @('0.0.0.0/0','0.0.0.0/1','128.0.0.0/1','::/0','::/1','8000::/1') } | ForEach-Object { "$($_.InterfaceIndex):$($_.DestinationPrefix):$($_.NextHop):$($_.RouteMetric)" } | Sort-Object)
$config = Join-Path $OutputDirectory 'probe.yaml'
# No proxy ports, no upstreams, no default routes, no DNS server, no WFP guard.
@'
mode: direct
log-level: debug
ipv6: true
tun:
  enable: true
  device: Atlas-TUN
  stack: mixed
  auto-route: false
  auto-detect-interface: false
  strict-route: false
  inet6-address: []
  dns-hijack: []
dns:
  enable: false
  fake-ip-range: 192.0.2.1/30
'@ | Set-Content -LiteralPath $config -Encoding utf8
$process = $null
try {
    $process = Start-Process -FilePath $binary -ArgumentList @('-d', ('"' + $OutputDirectory + '"'), '-f', ('"' + $config + '"')) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $OutputDirectory 'stdout.log') -RedirectStandardError (Join-Path $OutputDirectory 'stderr.log')
    $deadline = (Get-Date).AddSeconds(60)
    $adapter = $null
    do {
        Start-Sleep -Milliseconds 500
        $process.Refresh()
        $adapter = Get-NetAdapter -Name 'Atlas-TUN' -IncludeHidden -ErrorAction SilentlyContinue
    } while (-not $adapter -and -not $process.HasExited -and (Get-Date) -lt $deadline)
    [pscustomobject]@{ Found=[bool]$adapter; Status=$adapter.Status; InterfaceIndex=$adapter.ifIndex; CoreExited=$process.HasExited } | ConvertTo-Json | Set-Content (Join-Path $OutputDirectory 'result.json')
} finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
        $process.WaitForExit()
    }
    $routesAfter = @(Get-NetRoute | Where-Object { $_.DestinationPrefix -in @('0.0.0.0/0','0.0.0.0/1','128.0.0.0/1','::/0','::/1','8000::/1') } | ForEach-Object { "$($_.InterfaceIndex):$($_.DestinationPrefix):$($_.NextHop):$($_.RouteMetric)" } | Sort-Object)
    [pscustomobject]@{ RoutingUnchanged = -not [bool](Compare-Object $routesBefore $routesAfter); AdapterRemoved = -not [bool](Get-NetAdapter -Name 'Atlas-TUN' -IncludeHidden -ErrorAction SilentlyContinue) } | ConvertTo-Json | Set-Content (Join-Path $OutputDirectory 'cleanup.json')
}
