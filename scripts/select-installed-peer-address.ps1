$ErrorActionPreference = 'Stop'
# Get-NetIPConfiguration joins hidden adapters into a single typed NetAdapter
# property and fails when the hosted image returns multiple matches. The peer
# only needs a usable address on an IPv4 default route, not that adapter join.
$routes = @(Get-NetRoute -AddressFamily IPv4 -DestinationPrefix '0.0.0.0/0' |
    Sort-Object { $_.RouteMetric + $_.InterfaceMetric })
foreach ($route in $routes) {
    $addresses = @(Get-NetIPAddress -AddressFamily IPv4 -InterfaceIndex $route.InterfaceIndex |
        Where-Object { $_.AddressState -eq 'Preferred' -and -not $_.SkipAsSource })
    foreach ($address in $addresses) {
        $ip = [Net.IPAddress]::Parse($address.IPAddress)
        if ($ip.AddressFamily -eq [Net.Sockets.AddressFamily]::InterNetwork -and
            -not [Net.IPAddress]::IsLoopback($ip) -and
            $ip.ToString() -ne '0.0.0.0' -and -not $ip.ToString().StartsWith('169.254.')) {
            return $ip.ToString()
        }
    }
}
throw 'No usable default-route IPv4 address for the disposable VLESS peer'
