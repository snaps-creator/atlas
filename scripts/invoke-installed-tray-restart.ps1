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
$shellPid=[AtlasInstalledShell]::ShellPid()
Write-Output ('Limited tray helper context: '+[AtlasInstalledShell]::ShellDiagnostic())
if (-not $shellPid) { throw 'Runner notification area unavailable before real tray input' }
$shellLevel=[AtlasInstalledShell]::Integrity($shellPid)
[AtlasInstalledShell]::VerifyShell($shellPid)
if ($callerLevel -ne 8192 -or $ownerLevel -ne $callerLevel) {
    throw "Tray UI action requires matching medium integrity: helper=$callerLevel owner=$ownerLevel"
}
Write-Output "Verified tray input integrity: helper=$callerLevel owner=$ownerLevel shell=$shellLevel"
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
using System.Diagnostics;
using System.Security.Principal;
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
    [DllImport("user32.dll",SetLastError=true)] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    sealed class PhysicalCoordinates : IDisposable {
        readonly IntPtr previous;
        public PhysicalCoordinates() {
            previous=SetThreadDpiAwarenessContext(new IntPtr(-4));
            if(previous==IntPtr.Zero)throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Enter physical native input coordinates");
        }
        public void Dispose(){SetThreadDpiAwarenessContext(previous);}
    }
    [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint access,bool inherit,uint pid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool OpenProcessToken(IntPtr process,uint access,out IntPtr token);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool GetTokenInformation(IntPtr token,int type,IntPtr data,int length,out int needed);
    [DllImport("advapi32.dll")] static extern IntPtr GetSidSubAuthorityCount(IntPtr sid);
    [DllImport("advapi32.dll")] static extern IntPtr GetSidSubAuthority(IntPtr sid,uint index);
    public static int InputIntegrity(uint pid) {
        var process=OpenProcess(0x1000,false,pid);
        if(process==IntPtr.Zero)throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Inspect native input owner");
        try {
            IntPtr token;
            if(!OpenProcessToken(process,8,out token))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Inspect native input token");
            try {
                using(var owner=new WindowsIdentity(token))using(var caller=WindowsIdentity.GetCurrent())
                using(var target=Process.GetProcessById((int)pid))using(var current=Process.GetCurrentProcess()) {
                    if(owner.User!=caller.User || target.SessionId!=current.SessionId)throw new Exception("Native input owner belongs to another user or session");
                }
                var data=Marshal.AllocHGlobal(256);
                try {
                    int needed;
                    if(!GetTokenInformation(token,25,data,256,out needed))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Inspect native input integrity");
                    var sid=Marshal.ReadIntPtr(data);uint index=(uint)Marshal.ReadByte(GetSidSubAuthorityCount(sid))-1;
                    return Marshal.ReadInt32(GetSidSubAuthority(sid,index));
                } finally {Marshal.FreeHGlobal(data);}
            } finally {CloseHandle(token);}
        } finally {CloseHandle(process);}
    }
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
        using(var coordinates=new PhysicalCoordinates()){OpenPhysical(pid);}
    }
    static void OpenPhysical(uint pid) {
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
    static void Click(int x,int y) {
        if(!SetCursorPos(x,y))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Position cursor at verified native menu item");
        Point actual;
        if(!GetCursorPos(out actual) || actual.x!=x || actual.y!=y)throw new Exception("Native cursor did not reach the verified menu point");
        var inputs=new[]{new Input{mouse=new Mouse{flags=2}},new Input{mouse=new Mouse{flags=4}}};
        if(SendInput(2,inputs,Marshal.SizeOf(typeof(Input)))!=2)throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Native menu input rejected");
    }
    static bool ReadItem(IntPtr popup,IntPtr menu,uint index,string label,out Rect rect) {
        rect=new Rect();var name=new StringBuilder(1024);
        if(GetMenuString(menu,index,name,name.Capacity,0x400)==0 || name.ToString()!=label)return false;
        uint state=GetMenuState(menu,index,0x400);
        // Reject disabled/grayed items, separators and submenu parents.
        return state!=uint.MaxValue && (state&0x813)==0 && GetMenuItemRect(popup,menu,index,out rect)
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
                Rect candidate;if(!ReadItem(window,bar.menu,i,label,out candidate))continue;
                foundPopup=window;foundMenu=bar.menu;foundIndex=i;foundRect=candidate;matches++;
            }
            return true;
        },IntPtr.Zero);
        menuDiagnostic=observed.ToString();
        if(matches>1)throw new Exception("Ambiguous native tray action: "+Diagnostic);
        popup=foundPopup;menu=foundMenu;index=foundIndex;rect=foundRect;return matches==1;
    }
    public static bool HasMenuItem(uint pid,string label) {
        using(var coordinates=new PhysicalCoordinates()) {
            IntPtr popup,menu;uint index;Rect rect;return FindItem(pid,label,out popup,out menu,out index,out rect);
        }
    }
    public static bool TryClickMenu(uint pid,string label,int requiredIntegrity) {
        using(var coordinates=new PhysicalCoordinates()){return TryClickPhysicalMenu(pid,label,requiredIntegrity);}
    }
    static bool TryClickPhysicalMenu(uint pid,string label,int requiredIntegrity) {
        IntPtr popup,menu;uint index;Rect rect;
        if(!FindItem(pid,label,out popup,out menu,out index,out rect))return false;
        // Revalidate the same native menu and its current input target before
        // injecting input; neither a provider acknowledgement nor stale bounds
        // may select another window.
        var bar=new MenuBar{size=(uint)Marshal.SizeOf(typeof(MenuBar))};uint owner;
        GetWindowThreadProcessId(popup,out owner);
        if(owner!=pid || !IsWindowVisible(popup) || !GetMenuBarInfo(popup,-4,0,ref bar) || bar.menu!=menu || !ReadItem(popup,menu,index,label,out rect))return false;
        var point=new Point{x=rect.left+(rect.right-rect.left)/2,y=rect.top+(rect.bottom-rect.top)/2};
        var hit=WindowFromPoint(point);
        if(GetAncestor(hit,2)!=popup){menuDiagnostic+="; input target is obscured at "+point.x+","+point.y+" hit="+hit;return false;}
        // Mandatory at the actual input target, including the standalone
        // fixture. Installed acceptance always requires medium (8192).
        if(InputIntegrity(owner)!=requiredIntegrity || InputIntegrity((uint)Process.GetCurrentProcess().Id)!=requiredIntegrity) {
            menuDiagnostic+="; native input integrity mismatch";return false;
        }
        Click(point.x,point.y);return true;
    }
    public static void RestoreCursor(){if(cursorSaved){using(var coordinates=new PhysicalCoordinates()){SetCursorPos(originalCursor.x,originalCursor.y);cursorSaved=false;}}}
}
'@
[AtlasTrayAcceptance]::Open([uint32]$ProcessId)
$deadline=[DateTime]::UtcNow.AddSeconds(10)
do {
    if ([AtlasTrayAcceptance]::TryClickMenu([uint32]$ProcessId,$Name,8192)) {
        Write-Output ([AtlasTrayAcceptance]::Diagnostic)
        exit 0
    }
    Start-Sleep -Milliseconds 100
} while ([DateTime]::UtcNow -lt $deadline)
throw ('Installed Atlas native Restart menu unavailable: '+[AtlasTrayAcceptance]::Diagnostic)
} catch {
    # Detached limited helpers have no captured stderr. PowerShell formats an
    # unhandled error only after finally closes the transcript, losing its cause.
    $failureRecord=$_
    $diagnostic="Tray helper failure: $($failureRecord.Exception.ToString())`r`nError ID: $($failureRecord.FullyQualifiedErrorId)`r`nScript stack: $($failureRecord.ScriptStackTrace)"
    Write-Output $diagnostic
    if ($EvidencePath) {
        try { [IO.File]::WriteAllText($EvidencePath+'.error.txt',$diagnostic,[Text.UTF8Encoding]::new($false)) }
        catch { Write-Output "Cannot persist tray error sidecar: $($_.Exception.Message)" }
    }
    throw $failureRecord
} finally {
    if ('AtlasTrayAcceptance' -as [type]) {[AtlasTrayAcceptance]::RestoreCursor()}
    if ($EvidencePath) {Stop-Transcript | Out-Null}
}
