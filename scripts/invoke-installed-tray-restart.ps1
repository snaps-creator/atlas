param([Parameter(Mandatory=$true)][int]$ProcessId,[Parameter(Mandatory=$true)][string]$Name,[string]$EvidencePath,[string]$RegistrationPath)
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
    [DllImport("shell32.dll",SetLastError=true)] static extern int Shell_NotifyIconGetRect(ref Icon icon,out Rect rect);
    [DllImport("user32.dll",SetLastError=true)] static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll",SetLastError=true)] static extern bool SetCursorPos(int x,int y);
    [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
    [DllImport("user32.dll")] static extern bool EnumWindows(WindowCallback callback,IntPtr state);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window,out uint pid);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr window,StringBuilder name,int length);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr FindWindow(string name,string title);
    [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool QueryFullProcessImageName(IntPtr process,uint flags,StringBuilder path,ref uint length);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr window,out Rect rect);
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point point);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr window,uint flags);
    [DllImport("user32.dll",SetLastError=true)] static extern bool GetMenuBarInfo(IntPtr window,int obj,int item,ref MenuBar bar);
    [DllImport("user32.dll")] static extern int GetMenuItemCount(IntPtr menu);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetMenuString(IntPtr menu,uint item,StringBuilder text,int length,uint flags);
    [DllImport("user32.dll")] static extern uint GetMenuState(IntPtr menu,uint item,uint flags);
    [DllImport("user32.dll",SetLastError=true)] static extern bool GetMenuItemRect(IntPtr window,IntPtr menu,uint item,out Rect rect);
    public sealed class Registration {
        public uint ProcessId,Id;
        public long Window;
        public int Left,Top,Right,Bottom;
    }
    public static bool HasForegroundOwner(uint pid) {
        uint owner;GetWindowThreadProcessId(GetForegroundWindow(),out owner);return owner==pid;
    }
    public static void ActivateFixtureWindow(uint pid) {
        using(var coordinates=new PhysicalCoordinates()) {
            if(InputIntegrity(pid)!=InputIntegrity((uint)Process.GetCurrentProcess().Id))throw new Exception("Fixture input integrity mismatch");
            IntPtr found=IntPtr.Zero;int matches=0;
            EnumWindows((window,state)=> {
                uint owner;GetWindowThreadProcessId(window,out owner);
                var name=new StringBuilder(256);GetClassName(window,name,name.Capacity);
                if(owner==pid && IsWindowVisible(window) && name.ToString()=="AtlasTrayFixtureWindow"){found=window;matches++;}
                return true;
            },IntPtr.Zero);
            Rect rect;
            if(matches!=1 || !GetWindowRect(found,out rect))throw new Exception("Owned fixture input window missing or ambiguous");
            var point=new Point{x=rect.left+(rect.right-rect.left)/2,y=rect.top+(rect.bottom-rect.top)/2};
            if(GetAncestor(WindowFromPoint(point),2)!=found)throw new Exception("Fixture input window is obscured");
            if(!GetCursorPos(out originalCursor))throw new Exception("Fixture cursor unavailable");
            cursorSaved=true;
            SendMouseClick(point.x,point.y);
        }
    }
    public static void Open(uint pid) {
        Open(pid,QueryRegisteredIcon(pid));
    }
    public static Registration QueryRegisteredIcon(uint pid) {
        using(var coordinates=new PhysicalCoordinates()){return QueryPhysicalIcon(pid);}
    }
    static Registration QueryPhysicalIcon(uint pid) {
        InputIntegrity(pid); // Read-only same user/session verification.
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
        Rect rect=new Rect();uint registeredId=0;var failures=new StringBuilder();
        for(uint id=1;id<=16;id++) {
            var icon=new Icon{size=(uint)Marshal.SizeOf(typeof(Icon)),window=found,id=id};
            int result=Shell_NotifyIconGetRect(ref icon,out rect);
            if(result==0 && rect.right>rect.left && rect.bottom>rect.top) { registeredId=id;break; }
            failures.AppendFormat(" id={0}:0x{1:X8}/Win32={2}",id,result,Marshal.GetLastWin32Error());
        }
        if(registeredId==0)throw new Exception("Cannot resolve installed tray rectangle; HWND="+found+" identifierSize="+Marshal.SizeOf(typeof(Icon))+failures);
        return new Registration{ProcessId=pid,Window=found.ToInt64(),Id=registeredId,Left=rect.left,Top=rect.top,Right=rect.right,Bottom=rect.bottom};
    }
    public static void Open(uint pid,Registration registration) {
        using(var coordinates=new PhysicalCoordinates()){OpenPhysical(pid,registration);}
    }
    static void OpenPhysical(uint pid,Registration registration) {
        if(registration==null || registration.ProcessId!=pid || registration.Id<1 || registration.Id>16 || registration.Right<=registration.Left || registration.Bottom<=registration.Top)
            throw new Exception("Registered tray proof does not match the requested owner or valid rectangle");
        InputIntegrity(pid);
        var found=new IntPtr(registration.Window);uint owner;
        GetWindowThreadProcessId(found,out owner);
        var name=new StringBuilder(256);GetClassName(found,name,name.Capacity);
        if(owner!=pid || name.ToString()!="tray_icon_app")throw new Exception("Registered tray proof no longer identifies the live owner window");
        var live=QueryPhysicalIcon(pid);
        if(live.Window!=registration.Window || live.Id!=registration.Id)throw new Exception("Registered tray identity changed before input");
        var rect=new Rect{left=live.Left,top=live.Top,right=live.Right,bottom=live.Bottom};
        trayDiagnostic="tray HWND="+found+" rect="+rect.left+","+rect.top+","+rect.right+","+rect.bottom;
        Point cursor;
        if(!GetCursorPos(out cursor))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Runner input desktop cursor unavailable");
        if(!cursorSaved){originalCursor=cursor;cursorSaved=true;}
        if(rect.right>rect.left && rect.bottom>rect.top && !SetCursorPos(rect.left+(rect.right-rect.left)/2,rect.top+(rect.bottom-rect.top)/2))
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Position runner cursor at registered Atlas icon");
        uint shellPid,hitPid;
        GetWindowThreadProcessId(FindWindow("Shell_TrayWnd",null),out shellPid);
        GetWindowThreadProcessId(WindowFromPoint(new Point{x=rect.left+(rect.right-rect.left)/2,y=rect.top+(rect.bottom-rect.top)/2}),out hitPid);
        if(shellPid==0 || hitPid!=shellPid || InputIntegrity(shellPid)!=InputIntegrity((uint)Process.GetCurrentProcess().Id))throw new Exception("Registered icon point mismatch: shell="+shellPid+" hit="+hitPid+" caller="+InputIntegrity((uint)Process.GetCurrentProcess().Id)+" shellLevel="+(shellPid==0?0:InputIntegrity(shellPid)));
        var shellProcess=OpenProcess(0x1000,false,shellPid);
        if(shellProcess==IntPtr.Zero)throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Inspect actual notification input owner");
        try {
            var image=new StringBuilder(32768);uint length=32768;
            if(!QueryFullProcessImageName(shellProcess,0,image,ref length) || !String.Equals(image.ToString(),System.IO.Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Windows),"explorer.exe"),StringComparison.OrdinalIgnoreCase))throw new Exception("Notification input owner is not Windows Explorer");
        } finally {CloseHandle(shellProcess);}
        // Genuine shell input gives the normal foreground grant and callback.
        SendMouseClick(rect.left+(rect.right-rect.left)/2,rect.top+(rect.bottom-rect.top)/2,true);
    }
    static bool Click(uint pid,int x,int y) {
        if(!HasForegroundOwner(pid)){menuDiagnostic+="; native menu owner is not foreground";return false;}
        SendMouseClick(x,y);return true;
    }
    static void SendMouseClick(int x,int y,bool right=false) {
        if(!SetCursorPos(x,y))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),"Position cursor at verified native menu item");
        Point actual;
        if(!GetCursorPos(out actual) || actual.x!=x || actual.y!=y)throw new Exception("Native cursor did not reach the verified menu point");
        // SetCursorPos above already proved the physical position. Inject only
        // the ordered button transitions; a second normalized move can apply
        // another coordinate mapping on a mixed-DPI virtual desktop.
        var inputs=new[]{new Input{mouse=new Mouse{flags=right?8u:2u}},new Input{mouse=new Mouse{flags=right?16u:4u}}};
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
        return Click(pid,point.x,point.y);
    }
    public static void RestoreCursor(){if(cursorSaved){using(var coordinates=new PhysicalCoordinates()){SetCursorPos(originalCursor.x,originalCursor.y);cursorSaved=false;}}}
}
'@
if (-not $RegistrationPath) { throw 'Read-only parent registration proof required before installed tray input' }
$proofRoot=[IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\')+'\'
if (-not [IO.Path]::GetFullPath($RegistrationPath).StartsWith($proofRoot,[StringComparison]::OrdinalIgnoreCase)) {throw 'Tray registration proof must stay under RUNNER_TEMP'}
if ((Get-Item -LiteralPath $RegistrationPath).Length -gt 2048) {throw 'Tray registration proof exceeds limit'}
$proof=Get-Content -LiteralPath $RegistrationPath -Raw | ConvertFrom-Json
if (@($proof.PSObject.Properties.Name).Count -ne 7) {throw 'Invalid tray registration proof fields'}
$registration=[AtlasTrayAcceptance+Registration]::new()
foreach ($field in 'ProcessId','Id','Window','Left','Top','Right','Bottom') {
    if ($proof.PSObject.Properties.Name -notcontains $field) {throw "Missing tray registration proof field: $field"}
    $registration.$field=$proof.$field
}
[AtlasTrayAcceptance]::Open([uint32]$ProcessId,$registration)
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
