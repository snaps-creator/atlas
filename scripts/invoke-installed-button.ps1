param([Parameter(Mandatory=$true)][int]$ProcessId,[Parameter(Mandatory=$true)][string]$Name,[switch]$ToggleOff)
$ErrorActionPreference='Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Disposable GitHub Windows runner required' }
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$condition=[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ProcessIdProperty,$ProcessId)
$nameCondition=[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty,$Name)
$deadline=[DateTime]::UtcNow.AddSeconds(15)
do {
    $window=[System.Windows.Automation.AutomationElement]::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Children,$condition)
    if ($window) {
        $button=$window.FindFirst([System.Windows.Automation.TreeScope]::Descendants,$nameCondition)
        if ($button -and $button.Current.IsEnabled) {
            $pattern=$null
            if ($ToggleOff -and $button.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern,[ref]$pattern)) {
                if ($pattern.Current.ToggleState -ne [System.Windows.Automation.ToggleState]::On) { throw 'Expected the installed setting to be enabled before disabling it' }
                $pattern.Toggle()
                exit 0
            }
            if ($button.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern,[ref]$pattern)) {
                $pattern.Invoke()
                exit 0
            }
        }
    }
    Start-Sleep -Milliseconds 200
} while ([DateTime]::UtcNow -lt $deadline)
throw "Installed button is not accessible: $Name"
