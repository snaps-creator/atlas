param([Parameter(Mandatory=$true)][int]$ProcessId)
$ErrorActionPreference='Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Disposable GitHub Windows runner required' }
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class AtlasTrayAcceptance {
    delegate bool WindowCallback(IntPtr window,IntPtr state);
    [DllImport("user32.dll")] static extern bool EnumWindows(WindowCallback callback,IntPtr state);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window,out uint pid);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr window,StringBuilder name,int length);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr window,uint message,UIntPtr w,IntPtr l);
    public static void Open(uint pid) {
        IntPtr found=IntPtr.Zero;
        EnumWindows((window,state)=> {
            uint owner; GetWindowThreadProcessId(window,out owner);
            if(owner==pid) {
                var name=new StringBuilder(256); GetClassName(window,name,256);
                if(name.ToString()=="tray_icon_app") { found=window; return false; }
            }
            return true;
        },IntPtr.Zero);
        if(found==IntPtr.Zero) throw new Exception("Installed Atlas tray window not found");
        // tray-icon 0.25.1's shell callback, followed by the real native menu.
        if(!PostMessage(found,6002,UIntPtr.Zero,new IntPtr(0x205))) throw new Exception("Tray menu could not be opened");
    }
}
'@
[AtlasTrayAcceptance]::Open([uint32]$ProcessId)
$name=[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty,'Перезагрузить')
$owner=[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ProcessIdProperty,$ProcessId)
$condition=[System.Windows.Automation.AndCondition]::new($name,$owner)
$deadline=[DateTime]::UtcNow.AddSeconds(10)
do {
    $item=[System.Windows.Automation.AutomationElement]::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Descendants,$condition)
    if ($item -and $item.Current.IsEnabled) {
        $pattern=$null
        if ($item.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern,[ref]$pattern)) {
            $pattern.Invoke()
            exit 0
        }
    }
    Start-Sleep -Milliseconds 100
} while ([DateTime]::UtcNow -lt $deadline)
throw 'Installed Atlas Restart menu item was not accessible'
