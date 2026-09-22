$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
function Section($name, [scriptblock]$read) {
    $sectionStart = [Diagnostics.Stopwatch]::StartNew()
    Write-Output "`n=== $name ==="
    try { & $read | Format-List | Out-String -Width 240 | Write-Output }
    catch { Write-Output ("Unavailable: " + $_.Exception.Message) }
    finally { Write-Output ("Section completed in " + $sectionStart.ElapsedMilliseconds + ' ms') }
}
Section 'Time' { Get-Date -Format o }
Section 'Windows time synchronization status' { w32tm.exe /query /status }
Section 'Atlas service' {
    Get-CimInstance Win32_Service -Filter "Name='AtlasNetworkService'" |
        Select-Object Name, State, Status, ProcessId, ExitCode, StartMode
}
Section 'Atlas / Mihomo processes (no command lines)' {
    Get-CimInstance Win32_Process -Filter "Name='atlas.exe' OR Name='atlas-vpn.exe' OR Name='mihomo.exe' OR Name='verge-mihomo.exe' OR Name='verge-mihomo-alpha.exe' OR Name='clash-verge-service.exe'" |
        Select-Object Name, ProcessId, ParentProcessId, CreationDate
}
Section 'DNS listeners and owners (presence alone is not a conflict)' {
    $processNames = @{}
    Get-CimInstance Win32_Process | ForEach-Object { $processNames[[int]$_.ProcessId] = $_.Name }
    Get-NetTCPConnection -State Listen | Where-Object { $_.LocalPort -in @(53,11053) } |
        Select-Object @{Name='Transport';Expression={'TCP'}},LocalAddress,LocalPort,OwningProcess,
            @{Name='Process';Expression={$processNames[[int]$_.OwningProcess]}}
    Get-NetUDPEndpoint | Where-Object { $_.LocalPort -in @(53,11053) } |
        Select-Object @{Name='Transport';Expression={'UDP'}},LocalAddress,LocalPort,OwningProcess,
            @{Name='Process';Expression={$processNames[[int]$_.OwningProcess]}}
}
Section 'User system proxy' {
    Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' |
        Select-Object ProxyEnable,ProxyServer
}
Section 'Network adapters' {
    Get-NetAdapter -IncludeHidden | Select-Object Name, InterfaceDescription, ifIndex, Status, LinkSpeed
}
Section 'OS and resource pressure' {
    Get-CimInstance Win32_OperatingSystem | Select-Object Caption,Version,BuildNumber,LastBootUpTime,FreePhysicalMemory,TotalVisibleMemorySize
    Get-CimInstance Win32_Processor | Select-Object Name,LoadPercentage
    Get-Process | Where-Object {$_.ProcessName -match 'atlas|mihomo|clash'} |
        Select-Object ProcessName,Id,CPU,WorkingSet64,HandleCount,@{Name='ThreadCount';Expression={$_.Threads.Count}}
}
Section 'Adapter packet counters' {
    Get-NetAdapterStatistics | Select-Object Name,ReceivedBytes,SentBytes,ReceivedDiscardedPackets,OutboundDiscardedPackets,ReceivedPacketErrors,OutboundPacketErrors
}
Section 'Neighbor cache and network profile' {
    Get-NetNeighbor | Where-Object {$_.State -in @('Unreachable','Incomplete','Reachable','Stale')} |
        Select-Object -First 80 InterfaceIndex,IPAddress,State
    Get-NetConnectionProfile | Select-Object InterfaceAlias,NetworkCategory,IPv4Connectivity,IPv6Connectivity
}
Section 'Core TCP egress (no browsing names)' {
    $coreIds = @(Get-CimInstance Win32_Process -Filter "Name='mihomo.exe'" | Select-Object -ExpandProperty ProcessId)
    Get-NetTCPConnection | Where-Object { $_.OwningProcess -in $coreIds } |
        Select-Object OwningProcess,LocalAddress,LocalPort,RemoteAddress,RemotePort,State
}
Section 'Windows route lookup for core TCP peers (binding may override)' {
    $coreIds = @(Get-CimInstance Win32_Process -Filter "Name='mihomo.exe'" | Select-Object -ExpandProperty ProcessId)
    $peers = @(Get-NetTCPConnection | Where-Object { $_.OwningProcess -in $coreIds -and $_.RemoteAddress -notin @('0.0.0.0','127.0.0.1','::','::1') } |
        Select-Object -ExpandProperty RemoteAddress -Unique | Select-Object -First 16)
    foreach ($peer in $peers) {
        try {
            Find-NetRoute -RemoteIPAddress $peer | Select-Object @{Name='Peer';Expression={$peer}},
                InterfaceIndex,InterfaceAlias,IPAddress,DestinationPrefix,NextHop,RouteMetric
        } catch { Write-Output ("Route unavailable for " + $peer + ': ' + $_.Exception.Message) }
    }
}
Section 'IP, gateways, DNS and DHCP leases' {
    Get-CimInstance Win32_NetworkAdapterConfiguration -Filter 'IPEnabled=True' |
        Select-Object Description, InterfaceIndex, IPAddress, IPSubnet, DefaultIPGateway,
            DNSServerSearchOrder, DHCPEnabled, DHCPServer, DHCPLeaseObtained, DHCPLeaseExpires
}
Section 'DNS servers per interface' {
    Get-DnsClientServerAddress | Select-Object InterfaceAlias, InterfaceIndex, AddressFamily, ServerAddresses
}
Section 'IPv4 / IPv6 routes' {
    Get-NetRoute | Sort-Object InterfaceIndex, DestinationPrefix |
        Select-Object InterfaceAlias, InterfaceIndex, AddressFamily, DestinationPrefix, NextHop, RouteMetric, State
}
Section 'Interface metrics and DHCP state' {
    Get-NetIPInterface | Select-Object InterfaceAlias, InterfaceIndex, AddressFamily, ConnectionState, Dhcp, InterfaceMetric,NlMtu,WeakHostSend,WeakHostReceive
}
Section 'Recent DHCP events' {
    Get-WinEvent -FilterHashtable @{ LogName='Microsoft-Windows-Dhcp-Client/Admin'; StartTime=(Get-Date).AddDays(-1) } -MaxEvents 20 |
        Select-Object TimeCreated, Id, LevelDisplayName, Message
}
