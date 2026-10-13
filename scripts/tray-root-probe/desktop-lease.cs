using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
public sealed class AtlasDiagnosticDesktopLease : IDisposable {
    [DllImport("user32.dll")] static extern IntPtr GetProcessWindowStation();
    [DllImport("user32.dll")] static extern IntPtr GetThreadDesktop(uint thread);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("user32.dll",SetLastError=true)] static extern bool GetUserObjectSecurity(IntPtr handle,ref uint information,byte[] descriptor,uint length,out uint needed);
    [DllImport("user32.dll",SetLastError=true)] static extern bool SetUserObjectSecurity(IntPtr handle,ref uint information,byte[] descriptor);
    readonly IntPtr station,desktop;
    readonly byte[] stationOriginal,desktopOriginal;
    static byte[] Read(IntPtr handle) {
        uint information=4,needed;
        GetUserObjectSecurity(handle,ref information,null,0,out needed);
        if(needed==0 || needed>65536)throw new Win32Exception(Marshal.GetLastWin32Error(),"Inspect disposable desktop permissions");
        var result=new byte[needed];
        if(!GetUserObjectSecurity(handle,ref information,result,needed,out needed))throw new Win32Exception(Marshal.GetLastWin32Error());
        return result;
    }
    static void Write(IntPtr handle,byte[] descriptor) {
        uint information=4;
        if(!SetUserObjectSecurity(handle,ref information,descriptor))throw new Win32Exception(Marshal.GetLastWin32Error(),"Set disposable desktop permissions");
    }
    static byte[] Add(byte[] original,SecurityIdentifier sid) {
        var descriptor=new RawSecurityDescriptor(original,0);
        if(descriptor.DiscretionaryAcl==null)throw new Exception("Refusing a desktop with no explicit DACL");
        descriptor.DiscretionaryAcl.InsertAce(descriptor.DiscretionaryAcl.Count,new CommonAce(AceFlags.None,AceQualifier.AccessAllowed,0x10000000,sid,false,null));
        var result=new byte[descriptor.BinaryLength];descriptor.GetBinaryForm(result,0);return result;
    }
    public AtlasDiagnosticDesktopLease(string identity) {
        if(Environment.GetEnvironmentVariable("GITHUB_ACTIONS")!="true")throw new Exception("Disposable CI desktop required");
        station=GetProcessWindowStation();desktop=GetThreadDesktop(GetCurrentThreadId());
        stationOriginal=Read(station);desktopOriginal=Read(desktop);
        var sid=new SecurityIdentifier(identity);
        try {Write(station,Add(stationOriginal,sid));Write(desktop,Add(desktopOriginal,sid));}
        catch {Write(station,stationOriginal);throw;}
    }
    public void Dispose() {try{Write(desktop,desktopOriginal);}finally{Write(station,stationOriginal);}}
}
