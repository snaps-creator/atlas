using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Windows.Forms;
// Disposable fixture-owned icon only. It does not register or replace Atlas's
// icon; installed acceptance independently proves Atlas's actual registration.
public static class AtlasInstalledTrayPreflight {
    [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] struct Icon {
        public uint size;public IntPtr window;public uint id,flags,message;public IntPtr image;
        [MarshalAs(UnmanagedType.ByValTStr,SizeConst=128)] public string tip;
        public uint state,mask;
        [MarshalAs(UnmanagedType.ByValTStr,SizeConst=256)] public string info;
        public uint timeout;
        [MarshalAs(UnmanagedType.ByValTStr,SizeConst=64)] public string title;
        public uint infoFlags;public Guid guid;public IntPtr balloon;
    }
    [DllImport("shell32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool Shell_NotifyIconW(uint operation,ref Icon icon);
    public static void Verify() {
        if(Environment.GetEnvironmentVariable("GITHUB_ACTIONS")!="true")throw new Exception("Disposable CI required");
        var window=new NativeWindow();
        window.CreateHandle(new CreateParams{Caption="Atlas installed desktop preflight"});
        var data=new Icon{size=(uint)Marshal.SizeOf(typeof(Icon)),window=window.Handle,id=1,flags=7,message=0x8001,image=System.Drawing.SystemIcons.Application.Handle,tip="Atlas disposable desktop preflight",info="",title=""};
        try{if(!Shell_NotifyIconW(0,ref data)){int code=Marshal.GetLastWin32Error();throw new Win32Exception(code,"Ordinary shell notification registration failed (Win32="+code+")");}}
        finally{Shell_NotifyIconW(2,ref data);window.DestroyHandle();}
    }
}
