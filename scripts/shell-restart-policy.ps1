# The caller supplies an already-authorized key. This helper does not open or
# choose a machine key; tests use a private disposable HKCU key.
function Invoke-ShellRestartPolicyLease {
    param(
        [Parameter(Mandatory=$true)][Microsoft.Win32.RegistryKey]$Registry,
        [Parameter(Mandatory=$true)][scriptblock]$Body
    )
    $leaseName='AutoRestartShell'
    $leaseHadValue=$Registry.GetValueNames() -contains $leaseName
    if ($leaseHadValue) {
        $leaseOriginalValue=$Registry.GetValue($leaseName,$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $leaseOriginalKind=$Registry.GetValueKind($leaseName)
    }
    $Registry.SetValue($leaseName,0,[Microsoft.Win32.RegistryValueKind]::DWord)
    try {
        & $Body
    } finally {
        if ($leaseHadValue) { $Registry.SetValue($leaseName,$leaseOriginalValue,$leaseOriginalKind) }
        else { $Registry.DeleteValue($leaseName,$false) }
    }
}
