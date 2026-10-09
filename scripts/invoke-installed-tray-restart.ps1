param([Parameter(Mandatory=$true)][int]$ProcessId,[Parameter(Mandatory=$true)][string]$Name)
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
    [StructLayout(LayoutKind.Sequential)] struct Point {public int x,y;}
    [StructLayout(LayoutKind.Sequential)] struct Rect {public int left,top,right,bottom;}
    [StructLayout(LayoutKind.Sequential)] struct Icon {public uint size;public IntPtr window;public uint id;public Guid guid;}
    [DllImport("user32.dll",SetLastError=true)] static extern bool GetCursorPos(out Point point);
    [DllImport("shell32.dll")] static extern int Shell_NotifyIconGetRect(ref Icon icon,out Rect rect);
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
        Point cursor; bool cursorOk=GetCursorPos(out cursor);
        Console.WriteLine("Tray probe cursor="+cursorOk+" error="+Marshal.GetLastWin32Error());
        for(uint id=1;id<=8;id++) {
            var icon=new Icon {size=(uint)Marshal.SizeOf(typeof(Icon)),window=found,id=id}; Rect rect;
            int hr=Shell_NotifyIconGetRect(ref icon,out rect);
            Console.WriteLine("Tray probe id="+id+" hr="+hr+" rectangle="+rect.left+","+rect.top+","+rect.right+","+rect.bottom);
        }
        // tray-icon 0.25.1's shell callback, followed by the real native menu.
        if(!PostMessage(found,6002,UIntPtr.Zero,new IntPtr(0x205))) throw new Exception("Tray menu could not be opened");
    }
}
'@
$desktop=[System.Windows.Automation.AutomationElement]::RootElement
$buttons=$desktop.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty,[System.Windows.Automation.ControlType]::Button))
foreach($b in $buttons) {
 if($b.Current.ProcessId -eq $ProcessId) {continue}
 Write-Output ("Shell button: " + $b.Current.Name + " owner=" + $b.Current.ProcessId)
 if($b.Current.Name -in @('Show hidden icons','Notification Chevron','Show Hidden Icons')) {
  $pattern=$null
  if($b.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern,[ref]$pattern)) {$pattern.Invoke();Start-Sleep -Milliseconds 300}
 }
}
[AtlasTrayAcceptance]::Open([uint32]$ProcessId)
$menuName=[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty,$Name)
$owner=[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ProcessIdProperty,$ProcessId)
$condition=[System.Windows.Automation.AndCondition]::new($menuName,$owner)
$deadline=[DateTime]::UtcNow.AddSeconds(10)
do {
    $item=[System.Windows.Automation.AutomationElement]::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Descendants,$condition)
    if ($item -and $item.Current.IsEnabled) {
        $pattern=$null
        if ($item.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern,[ref]$pattern)) {
            $pattern.Invoke()
            exit 0
        }
        if ($item.TryGetCurrentPattern([System.Windows.Automation.LegacyIAccessiblePattern]::Pattern,[ref]$pattern)) {
            $pattern.DoDefaultAction()
            exit 0
        }
    }
    Start-Sleep -Milliseconds 100
} while ([DateTime]::UtcNow -lt $deadline)
$matches=[System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Descendants,$menuName)
$matches | ForEach-Object { [pscustomobject]@{menuOwner=$_.Current.ProcessId;control=$_.Current.ControlType.ProgrammaticName;patterns=@($_.GetSupportedPatterns() | ForEach-Object ProgrammaticName)} } | ConvertTo-Json -Depth 3 | Write-Output
throw 'Installed Atlas Restart menu item was not accessible'


