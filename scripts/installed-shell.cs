using System;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;

// Used only by the disposable installed acceptance script. No Atlas IPC or
// network settings are involved; the application stays at medium integrity.
public static class AtlasInstalledShell {
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] struct Startup {
        public uint size; public string reserved,desktop,title;
        public uint x,y,width,height,xChars,yChars,fill,flags;
        public ushort show,reservedSize; public IntPtr reservedData,input,output,error;
    }
    [StructLayout(LayoutKind.Sequential)] struct Created { public IntPtr process,thread;public uint pid,tid; }
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr FindWindow(string name,string title);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window,out uint pid);
    [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint access,bool inherit,uint pid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool OpenProcessToken(IntPtr process,uint access,out IntPtr token);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool GetTokenInformation(IntPtr token,int type,IntPtr data,int length,out int needed);
    [DllImport("advapi32.dll")] static extern IntPtr GetSidSubAuthorityCount(IntPtr sid);
    [DllImport("advapi32.dll")] static extern IntPtr GetSidSubAuthority(IntPtr sid,uint index);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool DuplicateTokenEx(IntPtr token,uint access,IntPtr attributes,int impersonation,int type,out IntPtr copy);
    [DllImport("advapi32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool CreateProcessWithTokenW(IntPtr token,uint flags,string app,StringBuilder command,uint creation,IntPtr environment,string directory,ref Startup startup,out Created process);
    [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool QueryFullProcessImageName(IntPtr process,uint flags,StringBuilder path,ref uint length);
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool TerminateProcess(IntPtr process,uint code);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr handle,uint timeout);
    static Exception Error(string action) { return new Win32Exception(Marshal.GetLastWin32Error(),action); }
    static IntPtr Token(IntPtr process) {
        IntPtr token;if(!OpenProcessToken(process,0xA,out token))throw Error("Inspect runner process token");return token;
    }
    static int Level(IntPtr token) {
        var data=Marshal.AllocHGlobal(256);
        try {
            int needed;if(!GetTokenInformation(token,25,data,256,out needed))throw Error("Inspect integrity level");
            var sid=Marshal.ReadIntPtr(data);uint index=(uint)Marshal.ReadByte(GetSidSubAuthorityCount(sid))-1;
            return Marshal.ReadInt32(GetSidSubAuthority(sid,index));
        } finally {Marshal.FreeHGlobal(data);}
    }
    static void SameUser(IntPtr token,uint pid) {
        using(var identity=new WindowsIdentity(token))using(var current=WindowsIdentity.GetCurrent()) {
            if(identity.User!=current.User || Process.GetProcessById((int)pid).SessionId!=Process.GetCurrentProcess().SessionId)
                throw new Exception("Refusing a different runner user or session");
        }
    }
    public static uint ShellPid() {
        uint pid;var window=FindWindow("Shell_TrayWnd",null);if(window==IntPtr.Zero)return 0;
        GetWindowThreadProcessId(window,out pid);return pid;
    }
    public static int Integrity(uint pid) {
        var process=OpenProcess(0x1000,false,pid);if(process==IntPtr.Zero)throw Error("Inspect process");
        try {var token=Token(process);try{SameUser(token,pid);return Level(token);}finally{CloseHandle(token);}}
        finally {CloseHandle(process);}
    }
    public static void StopVerifiedShell(uint pid) {
        var process=OpenProcess(0x101001,false,pid);if(process==IntPtr.Zero)throw Error("Open runner shell");
        try {
            var name=new StringBuilder(32768);uint length=32768;
            if(!QueryFullProcessImageName(process,0,name,ref length))throw Error("Inspect shell image");
            if(!String.Equals(name.ToString(),Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Windows),"explorer.exe"),StringComparison.OrdinalIgnoreCase))
                throw new Exception("Refusing a non-Explorer process");
            var token=Token(process);
            try {SameUser(token,pid);if(Level(token)<=8192)throw new Exception("Refusing to replace a limited shell");}
            finally {CloseHandle(token);}
            if(!TerminateProcess(process,0))throw Error("Stop elevated runner shell");
            if(WaitForSingleObject(process,5000)!=0)throw new Exception("Runner shell did not exit");
        } finally {CloseHandle(process);}
    }
    public static void Launch(uint desktopPid) {
        var process=OpenProcess(0x1000,false,desktopPid);if(process==IntPtr.Zero)throw Error("Open desktop owner");
        try {
            var token=Token(process);
            try {
                SameUser(token,desktopPid);if(Level(token)!=8192)throw new Exception("Desktop token is not medium");
                using(var identity=new WindowsIdentity(token))if(new WindowsPrincipal(identity).IsInRole(WindowsBuiltInRole.Administrator))
                    throw new Exception("Desktop token still grants administrative access");
                IntPtr copy;if(!DuplicateTokenEx(token,0x18B,IntPtr.Zero,2,1,out copy))throw Error("Duplicate desktop token");
                try {
                    string directory=Environment.GetFolderPath(Environment.SpecialFolder.Windows),exe=Path.Combine(directory,"explorer.exe");
                    var startup=new Startup{size=(uint)Marshal.SizeOf(typeof(Startup)),desktop="winsta0\\default"};Created created;
                    if(!CreateProcessWithTokenW(copy,0,exe,new StringBuilder("\""+exe+"\""),0,IntPtr.Zero,directory,ref startup,out created))throw Error("Launch limited notification area");
                    CloseHandle(created.thread);CloseHandle(created.process);
                } finally {CloseHandle(copy);}
            } finally {CloseHandle(token);}
        } finally {CloseHandle(process);}
    }
}
