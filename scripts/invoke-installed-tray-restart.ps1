param([Parameter(Mandatory=$true)][int]$ProcessId,[Parameter(Mandatory=$true)][string]$Name,[string]$EvidencePath)
$ErrorActionPreference='Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Disposable GitHub Windows runner required' }
if ($EvidencePath) {
    $allowed=[IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')+'\'
    if (-not [IO.Path]::GetFullPath($EvidencePath).StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase)) {throw 'Tray evidence must stay under RUNNER_TEMP'}
    Start-Transcript -LiteralPath $EvidencePath -Force | Out-Null
}
try {
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -Path (Join-Path $PSScriptRoot 'installed-shell.cs')
$callerLevel=[AtlasInstalledShell]::Integrity([uint32]$PID)
$ownerLevel=[AtlasInstalledShell]::Integrity([uint32]$ProcessId)
$shellLevel=[AtlasInstalledShell]::Integrity([AtlasInstalledShell]::ShellPid())
if ($callerLevel -ne 8192 -or $ownerLevel -ne $callerLevel -or $shellLevel -ne $callerLevel) {
    throw "Tray UI action requires matching medium integrity: helper=$callerLevel owner=$ownerLevel shell=$shellLevel"
}
Write-Output "Verified tray input integrity: helper=$callerLevel owner=$ownerLevel shell=$shellLevel"
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class AtlasTrayAcceptance {
    delegate bool WindowCallback(IntPtr window,IntPtr state);
    [StructLayout(LayoutKind.Sequential)] struct Point { public int x,y; }
    [StructLayout(LayoutKind.Sequential)] struct Rect { public int left,top,right,bottom; }
    [StructLayout(LayoutKind.Sequential)] struct Icon { public uint size;public IntPtr window;public uint id;public Guid guid; }
    [StructLayout(LayoutKind.Sequential)] struct Mouse { public int x,y;public uint data,flags,time;public UIntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct Input { public uint type;public Mouse mouse; }
    static Point originalCursor;
    static bool cursorSaved;
    [DllImport("user32.dll",SetLastError=true)] static extern uint SendInput(uint count,Input[] inputs,int size);
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
        Rect rect=new Rect();bool registered=false;var failures=new StringBuilder();
        for(uint id=1;id<=16;id++) {
            var icon=new Icon{size=(uint)Marshal.SizeOf(typeof(Icon)),window=found,id=id};
            int result=Shell_NotifyIconGetRect(ref icon,out rect);
            if(result==0) { registered=true;break; }
            failures.AppendFormat(" id={0}:0x{1:X8}",id,result);
        }
        if(!registered)throw new Exception("Cannot resolve installed tray rectangle; HWND="+found+" identifierSize="+Marshal.SizeOf(typeof(Icon))+failures);
        Point cursor;
        if(!GetCursorPos(out cursor))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Runner input desktop cursor unavailable");
        originalCursor=cursor;cursorSaved=true;
        if(rect.right>rect.left && rect.bottom>rect.top && !SetCursorPos(rect.left+(rect.right-rect.left)/2,rect.top+(rect.bottom-rect.top)/2))
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Position runner cursor at registered Atlas icon");
        // tray-icon 0.25.1's shell callback, followed by the real native menu.
        if(!PostMessage(found,6002,UIntPtr.Zero,new IntPtr(0x205))) throw new Exception("Tray menu could not be opened");
    }
    public static void Click(int x,int y) {
        if(!SetCursorPos(x,y))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Position cursor at verified native menu item");
        var inputs=new[]{new Input{mouse=new Mouse{flags=2}},new Input{mouse=new Mouse{flags=4}}};
        if(SendInput(2,inputs,Marshal.SizeOf(typeof(Input)))!=2)throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Native menu input rejected");
    }
    public static void RestoreCursor(){if(cursorSaved){SetCursorPos(originalCursor.x,originalCursor.y);cursorSaved=false;}}
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
        # The Win32 provider's Invoke can return without WM_COMMAND. Use real
        # input on the verified owner's actual popup, then the parent harness
        # requires an actual restart/new PID and durable settings preservation.
        if ($item.Current.ControlType -ne [System.Windows.Automation.ControlType]::MenuItem) {throw 'Tray action did not resolve to a native menu item'}
        $bounds=$item.Current.BoundingRectangle
        if ($bounds.IsEmpty -or $bounds.Width -le 0 -or $bounds.Height -le 0) {throw 'Native tray menu item has no clickable rectangle'}
        [AtlasTrayAcceptance]::Click([int]($bounds.Left+$bounds.Width/2),[int]($bounds.Top+$bounds.Height/2))
        exit 0
    }
    Start-Sleep -Milliseconds 100
} while ([DateTime]::UtcNow -lt $deadline)
$matches=[System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Descendants,$menuName)
$matches | ForEach-Object { [pscustomobject]@{menuOwner=$_.Current.ProcessId;control=$_.Current.ControlType.ProgrammaticName;patterns=@($_.GetSupportedPatterns() | ForEach-Object ProgrammaticName)} } | ConvertTo-Json -Depth 3 | Write-Output
throw 'Installed Atlas Restart menu item was not accessible'
} finally {
    if ('AtlasTrayAcceptance' -as [type]) {[AtlasTrayAcceptance]::RestoreCursor()}
    if ($EvidencePath) {Stop-Transcript | Out-Null}
}
