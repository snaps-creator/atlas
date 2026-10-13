$ErrorActionPreference='Stop'
if($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows'){throw 'Disposable GitHub Windows runner required'}
Add-Type -Path (Join-Path $PSScriptRoot 'installed-shell.cs')
$shell=[AtlasInstalledShell]::ShellPid();[AtlasInstalledShell]::VerifyShell($shell)
if([AtlasInstalledShell]::Integrity($shell) -ne 8192){throw 'Notification overflow requires the ordinary fixture shell'}
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes
$condition=[Windows.Automation.AndCondition]::new(
    [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,[int]$shell),
    [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ClassNameProperty,'Shell_TrayWnd'))
$taskbar=[Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children,$condition)
if(-not $taskbar){throw 'Verified taskbar automation element missing'}
$buttons=$taskbar.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Button))
$chevrons=@($buttons | Where-Object {$_.Current.Name -in 'Notification Chevron','Show hidden icons'})
if($chevrons.Count -eq 0){Write-Output ('No overflow button; stock shell buttons: '+(@($buttons | Select-Object -First 20 | ForEach-Object {$_.Current.Name+' ['+$_.Current.AutomationId+']'}) -join '; '));return}
if($chevrons.Count -ne 1 -or -not $chevrons[0].Current.IsEnabled){throw 'Notification overflow button is ambiguous or disabled'}
$pattern=$null
if(-not $chevrons[0].TryGetCurrentPattern([Windows.Automation.InvokePattern]::Pattern,[ref]$pattern)){throw 'Stock notification overflow action unavailable'}
$pattern.Invoke()
$popupCondition=[Windows.Automation.AndCondition]::new(
    [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,[int]$shell),
    [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ClassNameProperty,'NotifyIconOverflowWindow'))
$deadline=[DateTime]::UtcNow.AddSeconds(3)
do {
    $popup=[Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children,$popupCondition)
    if($popup -and -not $popup.Current.IsOffscreen){Write-Output 'PASS: stock notification overflow is visible';return}
    Start-Sleep -Milliseconds 50
} while([DateTime]::UtcNow -lt $deadline)
throw 'Stock notification overflow did not become visible'
