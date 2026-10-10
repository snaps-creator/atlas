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
    [StructLayout(LayoutKind.Sequential)] struct Point { public int x,y; }
    [StructLayout(LayoutKind.Sequential)] struct Rect { public int left,top,right,bottom; }
    [StructLayout(LayoutKind.Sequential)] struct Icon { public uint size;public IntPtr window;public uint id;public Guid guid; }
    [DllImport("shell32.dll")] static extern int Shell_NotifyIconGetRect(ref Icon icon,out Rect rect);
    [DllImport("user32.dll",SetLastError=true)] static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll",SetLastError=true)] static extern bool SetCursorPos(int x,int y);
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
        // A callback enqueue alone does not prove the shell registered the icon.
        // Query only this verified owner's tray HWND, with a bounded id inventory.
        Rect rect=new Rect();bool registered=false;
        for(uint id=1;id<=16;id++) {
            var icon=new Icon{size=(uint)Marshal.SizeOf(typeof(Icon)),window=found,id=id};
            if(Shell_NotifyIconGetRect(ref icon,out rect)>=0) { registered=true;break; }
        }
        if(!registered)throw new Exception("Installed Atlas tray icon is not registered in the runner notification area");
        Point cursor;
        if(!GetCursorPos(out cursor))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Runner input desktop cursor unavailable");
        if(rect.right>rect.left && rect.bottom>rect.top && !SetCursorPos(rect.left+(rect.right-rect.left)/2,rect.top+(rect.bottom-rect.top)/2))
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Position runner cursor at registered Atlas icon");
        // tray-icon 0.25.1's shell callback, followed by the real native menu.
        if(!PostMessage(found,6002,UIntPtr.Zero,new IntPtr(0x205))) throw new Exception("Tray menu could not be opened");
    }
}
'@
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
