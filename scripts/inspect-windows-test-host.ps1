param([Parameter(Mandatory=$true)][string]$OutputPath)
$ErrorActionPreference = 'Stop'
$features = foreach ($featureName in @('Microsoft-Hyper-V-All', 'Containers-DisposableClientVM')) {
    $feature = Get-WindowsOptionalFeature -Online -FeatureName $featureName
    [ordered]@{ name = $featureName; state = [string]$feature.State }
}
$computer = Get-CimInstance Win32_ComputerSystem
$result = [ordered]@{
    features = @($features)
    hypervisorPresent = $computer.HypervisorPresent
    memoryBytes = $computer.TotalPhysicalMemory
    elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}
$result | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $OutputPath -Encoding utf8
