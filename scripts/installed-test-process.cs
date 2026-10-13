using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;
public sealed class AtlasInstalledTestProcess : IDisposable {
    [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] struct Startup {public uint size;public string reserved,desktop,title;public uint x,y,cx,cy,charsX,charsY,fill,flags;public ushort show,bytes;public IntPtr extra,input,output,error;}
    [StructLayout(LayoutKind.Sequential)] struct Created {public IntPtr process,thread;public uint pid,tid;}
    [DllImport("advapi32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool LogonUser(string name,string domain,string password,uint type,uint provider,out IntPtr token);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool GetTokenInformation(IntPtr token,int kind,IntPtr data,int size,out int needed);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool DuplicateTokenEx(IntPtr token,uint access,IntPtr attributes,int impersonation,int type,out IntPtr copy);
    [DllImport("advapi32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool CreateProcessWithTokenW(IntPtr token,uint logon,string application,StringBuilder command,uint flags,IntPtr environment,string directory,ref Startup startup,out Created process);
    [DllImport("userenv.dll",SetLastError=true)] static extern bool CreateEnvironmentBlock(out IntPtr environment,IntPtr token,bool inherit);
    [DllImport("userenv.dll")] static extern bool DestroyEnvironmentBlock(IntPtr environment);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr process,uint timeout);
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetExitCodeProcess(IntPtr process,out uint code);
    [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr process,uint code);
    [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint access,bool inherit,uint pid);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool OpenProcessToken(IntPtr process,uint access,out IntPtr token);
    [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool QueryFullProcessImageName(IntPtr process,uint flags,StringBuilder path,ref uint length);
    IntPtr process;
    public uint Id {get;private set;}
    static Win32Exception Error(string action){int code=Marshal.GetLastWin32Error();return new Win32Exception(code,action+" (Win32="+code+")");}
    static int Elevation(IntPtr token) {
        var data=Marshal.AllocHGlobal(4);try{int needed;if(!GetTokenInformation(token,18,data,4,out needed))throw Error("Inspect diagnostic elevation");return Marshal.ReadInt32(data);}finally{Marshal.FreeHGlobal(data);}
    }
    public AtlasInstalledTestProcess(string name,string identity,string password,string application,string command,string directory) {
        if(Environment.GetEnvironmentVariable("GITHUB_ACTIONS")!="true" || !System.Text.RegularExpressions.Regex.IsMatch(name,"^AtlasProbe[0-9a-f]{8}$"))throw new Exception("Private disposable CI identity required");
        IntPtr token;if(!LogonUser(name,".",password,4,0,out token))throw Error("Log on private administrative batch worker");
        IntPtr linked=IntPtr.Zero,primary=IntPtr.Zero;
        try {
            var selected=token;
            using(var user=new WindowsIdentity(selected)) {
                if(user.User.Value!=identity || !new WindowsPrincipal(user).IsInRole(WindowsBuiltInRole.Administrator) || Elevation(selected)==3)throw new Exception("Credential worker requires the exact private user's administrative batch token");
            }
            if(!DuplicateTokenEx(selected,0x18B,IntPtr.Zero,2,1,out primary))throw Error("Prepare private primary installer token");
            IntPtr environment;if(!CreateEnvironmentBlock(out environment,primary,false))throw Error("Prepare private profile environment");
            try {
                var expected=System.IO.Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles),"PowerShell/7/pwsh.exe");
                if(!String.Equals(System.IO.Path.GetFullPath(application),System.IO.Path.GetFullPath(expected),StringComparison.OrdinalIgnoreCase))throw new Exception("Installation worker requires the trusted PowerShell 7 host");
                var app=application;
                var startup=new Startup{size=(uint)Marshal.SizeOf(typeof(Startup)),desktop="winsta0\\default",flags=1,show=0};Created child;
                if(!CreateProcessWithTokenW(primary,1,app,new StringBuilder("\""+app+"\" "+command),0x08000400,environment,directory,ref startup,out child))throw Error("Start private elevated installer worker");
                CloseHandle(child.thread);process=child.process;Id=child.pid;
                using(var created=Process.GetProcessById((int)Id))using(var caller=Process.GetCurrentProcess())if(created.SessionId!=caller.SessionId)throw new Exception("Diagnostic worker belongs to another session");
            }finally{DestroyEnvironmentBlock(environment);}
        }finally{if(primary!=IntPtr.Zero)CloseHandle(primary);if(linked!=IntPtr.Zero)CloseHandle(linked);CloseHandle(token);}
    }
    public bool WaitForExit(int timeout){if(timeout<1 || timeout>2100000)throw new Exception("Invalid diagnostic wait");return WaitForSingleObject(process,(uint)timeout)==0;}
    public uint ExitCode {get{uint code;if(!GetExitCodeProcess(process,out code))throw Error("Inspect diagnostic exit");return code;}}
    public static void StopPrivateShell(uint pid,string identity) {
        var handle=OpenProcess(0x101001,false,pid);if(handle==IntPtr.Zero)throw Error("Inspect private test shell");
        try {
            var path=new StringBuilder(32768);uint length=32768;
            if(!QueryFullProcessImageName(handle,0,path,ref length) || !String.Equals(path.ToString(),System.IO.Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Windows),"explorer.exe"),StringComparison.OrdinalIgnoreCase))throw new Exception("Refusing a non-Explorer private shell");
            using(var target=Process.GetProcessById((int)pid))using(var caller=Process.GetCurrentProcess())if(target.SessionId!=caller.SessionId)throw new Exception("Private shell belongs to another session");
            IntPtr token;if(!OpenProcessToken(handle,8,out token))throw Error("Inspect private shell identity");
            try{using(var owner=new WindowsIdentity(token))if(owner.User.Value!=identity)throw new Exception("Private shell does not belong to the disposable account");}finally{CloseHandle(token);}
            if(!TerminateProcess(handle,0) || WaitForSingleObject(handle,5000)!=0)throw Error("Stop private test shell");
        }finally{CloseHandle(handle);}
    }
    public void Dispose(){if(process!=IntPtr.Zero){if(WaitForSingleObject(process,0)!=0){TerminateProcess(process,1);WaitForSingleObject(process,5000);}CloseHandle(process);process=IntPtr.Zero;}}
}
