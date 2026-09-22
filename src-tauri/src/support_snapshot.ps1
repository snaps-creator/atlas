$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
function Section($name, [scriptblock]$read) {
    Write-Output "`n=== $name ==="
    try { & $read | Format-List | Out-String -Width 240 | Write-Output }
    catch { Write-Output ("Unavailable: " + $_.Exception.Message) }
}
Section 'Time' { Get-Date -Format o }
Section 'Atlas service' {
    Get-CimInstance Win32_Service -Filter "Name='AtlasNetworkService'" |
        Select-Object Name, State, Status, ProcessId, ExitCode, StartMode
}
Section 'Atlas / Mihomo processes (no command lines)' {
    Get-CimInstance Win32_Process -Filter "Name='atlas.exe' OR Name='atlas-vpn.exe' OR Name='mihomo.exe'" |
        Select-Object Name, ProcessId, ParentProcessId, CreationDate
}
Section 'Network adapters' {
    Get-NetAdapter -IncludeHidden | Select-Object Name, InterfaceDescription, ifIndex, Status, LinkSpeed
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
    Get-NetIPInterface | Select-Object InterfaceAlias, InterfaceIndex, AddressFamily, ConnectionState, Dhcp, InterfaceMetric
}
Section 'Recent DHCP events' {
    Get-WinEvent -FilterHashtable @{ LogName='Microsoft-Windows-Dhcp-Client/Admin'; StartTime=(Get-Date).AddDays(-1) } -MaxEvents 20 |
        Select-Object TimeCreated, Id, LevelDisplayName, Message
}
