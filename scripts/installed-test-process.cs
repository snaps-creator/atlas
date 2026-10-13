using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;
// Cleanup applies only to the exact disposable user's Explorer in this session.
public static class AtlasInstalledTestProcess {
    [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint access,bool inherit,uint pid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr process,uint code);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr process,uint timeout);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool OpenProcessToken(IntPtr process,uint access,out IntPtr token);
    [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool QueryFullProcessImageName(IntPtr process,uint flags,StringBuilder path,ref uint length);
    static Win32Exception Error(string action){int code=Marshal.GetLastWin32Error();return new Win32Exception(code,action+" (Win32="+code+")");}
    public static void StopPrivateShell(uint pid,string identity) {
        if(Environment.GetEnvironmentVariable("GITHUB_ACTIONS")!="true")throw new Exception("Disposable GitHub Windows runner required");
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
}
