param([Parameter(Mandatory=$true)][int]$ProcessId,[Parameter(Mandatory=$true)][string]$Name,[string]$EvidencePath)
$ErrorActionPreference='Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') { throw 'Disposable GitHub Windows runner required' }
if ($EvidencePath) {
    $allowed=[IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')+'\'
    if (-not [IO.Path]::GetFullPath($EvidencePath).StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase)) {throw 'Tray evidence must stay under RUNNER_TEMP'}
    Start-Transcript -LiteralPath $EvidencePath -Force | Out-Null
}
try {
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
    [StructLayout(LayoutKind.Sequential)] struct MenuBar { public uint size;public Rect rect;public IntPtr menu,window;public uint flags; }
    static Point originalCursor;
    static bool cursorSaved;
    static string trayDiagnostic="",menuDiagnostic="";
    public static string Diagnostic { get {return trayDiagnostic+"; "+menuDiagnostic;} }
    [DllImport("user32.dll",SetLastError=true)] static extern uint SendInput(uint count,Input[] inputs,int size);
    [DllImport("shell32.dll")] static extern int Shell_NotifyIconGetRect(ref Icon icon,out Rect rect);
    [DllImport("user32.dll",SetLastError=true)] static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll",SetLastError=true)] static extern bool SetCursorPos(int x,int y);
    [DllImport("user32.dll")] static extern bool EnumWindows(WindowCallback callback,IntPtr state);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window,out uint pid);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr window,StringBuilder name,int length);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr window,uint message,UIntPtr w,IntPtr l);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point point);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr window,uint flags);
    [DllImport("user32.dll",SetLastError=true)] static extern bool GetMenuBarInfo(IntPtr window,int obj,int item,ref MenuBar bar);
    [DllImport("user32.dll")] static extern int GetMenuItemCount(IntPtr menu);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetMenuString(IntPtr menu,uint item,StringBuilder text,int length,uint flags);
    [DllImport("user32.dll")] static extern uint GetMenuState(IntPtr menu,uint item,uint flags);
    [DllImport("user32.dll",SetLastError=true)] static extern bool GetMenuItemRect(IntPtr window,IntPtr menu,uint item,out Rect rect);
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
        trayDiagnostic="tray HWND="+found+" rect="+rect.left+","+rect.top+","+rect.right+","+rect.bottom;
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
    static bool ReadItem(IntPtr menu,uint index,string label,out Rect rect) {
        rect=new Rect();var name=new StringBuilder(1024);
        if(GetMenuString(menu,index,name,name.Capacity,0x400)==0 || name.ToString()!=label)return false;
        uint state=GetMenuState(menu,index,0x400);
        // Reject disabled/grayed items, separators and submenu parents.
        return state!=uint.MaxValue && (state&0x813)==0 && GetMenuItemRect(IntPtr.Zero,menu,index,out rect)
            && rect.right>rect.left && rect.bottom>rect.top;
    }
    static bool FindItem(uint pid,string label,out IntPtr popup,out IntPtr menu,out uint index,out Rect rect) {
        IntPtr foundPopup=IntPtr.Zero,foundMenu=IntPtr.Zero;uint foundIndex=0;Rect foundRect=new Rect();int matches=0;
        var observed=new StringBuilder();uint foregroundPid;var foreground=GetForegroundWindow();GetWindowThreadProcessId(foreground,out foregroundPid);
        observed.AppendFormat("target PID={0}; foreground HWND={1} PID={2}",pid,foreground,foregroundPid);
        EnumWindows((window,state)=>{
            uint owner;GetWindowThreadProcessId(window,out owner);
            if(owner!=pid || !IsWindowVisible(window))return true;
            var cls=new StringBuilder(256);GetClassName(window,cls,cls.Capacity);
            if(cls.ToString()!="#32768")return true;
            var bar=new MenuBar{size=(uint)Marshal.SizeOf(typeof(MenuBar))};
            if(!GetMenuBarInfo(window,-4,0,ref bar)){observed.AppendFormat("; popup={0} GetMenuBarInfo error={1}",window,Marshal.GetLastWin32Error());return true;}
            int count=GetMenuItemCount(bar.menu);observed.AppendFormat("; popup={0} HMENU={1} count={2}",window,bar.menu,count);
            if(count<1 || count>32)return true;
            for(uint i=0;i<count;i++) {
                var name=new StringBuilder(256);GetMenuString(bar.menu,i,name,name.Capacity,0x400);
                observed.AppendFormat(" [{0}:{1} state=0x{2:X}]",i,name,GetMenuState(bar.menu,i,0x400));
                Rect candidate;if(!ReadItem(bar.menu,i,label,out candidate))continue;
                foundPopup=window;foundMenu=bar.menu;foundIndex=i;foundRect=candidate;matches++;
            }
            return true;
        },IntPtr.Zero);
        menuDiagnostic=observed.ToString();
        if(matches>1)throw new Exception("Ambiguous native tray action: "+Diagnostic);
        popup=foundPopup;menu=foundMenu;index=foundIndex;rect=foundRect;return matches==1;
    }
    public static bool HasMenuItem(uint pid,string label) {
        IntPtr popup,menu;uint index;Rect rect;return FindItem(pid,label,out popup,out menu,out index,out rect);
    }
    public static bool TryClickMenu(uint pid,string label) {
        IntPtr popup,menu;uint index;Rect rect;
        if(!FindItem(pid,label,out popup,out menu,out index,out rect))return false;
        // Revalidate the same native menu and its current input target before
        // injecting input; neither a provider acknowledgement nor stale bounds
        // may select another window.
        var bar=new MenuBar{size=(uint)Marshal.SizeOf(typeof(MenuBar))};uint owner;
        GetWindowThreadProcessId(popup,out owner);
        if(owner!=pid || !IsWindowVisible(popup) || !GetMenuBarInfo(popup,-4,0,ref bar) || bar.menu!=menu || !ReadItem(menu,index,label,out rect))return false;
        var point=new Point{x=rect.left+(rect.right-rect.left)/2,y=rect.top+(rect.bottom-rect.top)/2};
        var hit=WindowFromPoint(point);
        if(GetAncestor(hit,2)!=popup){menuDiagnostic+="; input target is obscured";return false;}
        Click(point.x,point.y);return true;
    }
    public static void RestoreCursor(){if(cursorSaved){SetCursorPos(originalCursor.x,originalCursor.y);cursorSaved=false;}}
}
'@
[AtlasTrayAcceptance]::Open([uint32]$ProcessId)
$deadline=[DateTime]::UtcNow.AddSeconds(10)
do {
    if ([AtlasTrayAcceptance]::TryClickMenu([uint32]$ProcessId,$Name)) {
        Write-Output ([AtlasTrayAcceptance]::Diagnostic)
        exit 0
    }
    Start-Sleep -Milliseconds 100
} while ([DateTime]::UtcNow -lt $deadline)
throw ('Installed Atlas native Restart menu unavailable: '+[AtlasTrayAcceptance]::Diagnostic)
} finally {
    if ('AtlasTrayAcceptance' -as [type]) {[AtlasTrayAcceptance]::RestoreCursor()}
    if ($EvidencePath) {Stop-Transcript | Out-Null}
}
