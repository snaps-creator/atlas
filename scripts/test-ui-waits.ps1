param([Parameter(Mandatory=$true)][int]$ProcessId,[Parameter(Mandatory=$true)][string]$Executable)
$ErrorActionPreference='Stop'
$process=Get-Process -Id $ProcessId -ErrorAction Stop
if ($process.Path -ne (Resolve-Path -LiteralPath $Executable).Path) {throw 'Refusing to inspect a process outside the isolated UI fixture'}
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class AtlasWaitEvidence {
    [DllImport("advapi32.dll",SetLastError=true)] static extern IntPtr OpenThreadWaitChainSession(uint flags,IntPtr callback);
    [DllImport("advapi32.dll")] static extern void CloseThreadWaitChainSession(IntPtr session);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool GetThreadWaitChain(IntPtr session,UIntPtr context,uint flags,uint tid,ref uint count,IntPtr nodes,out bool cycle);
    public static object Read(uint tid) {
        var session=OpenThreadWaitChainSession(0,IntPtr.Zero);
        if(session==IntPtr.Zero) return new {thread=tid,error=Marshal.GetLastWin32Error()};
        var buffer=Marshal.AllocHGlobal(280*16);
        try {
            uint count=16; bool cycle;
            bool ok=GetThreadWaitChain(session,UIntPtr.Zero,0,tid,ref count,buffer,out cycle);
            int error=ok?0:Marshal.GetLastWin32Error();
            var nodes=new List<object>();
            if(ok) for(int i=0;i<count && i<16;i++) {
                var node=IntPtr.Add(buffer,i*280);
                int type=Marshal.ReadInt32(node,0),status=Marshal.ReadInt32(node,4);
                // Object names and process memory are intentionally excluded.
                nodes.Add(new {type=type,status=status,pid=type==8?Marshal.ReadInt32(node,8):0,tid=type==8?Marshal.ReadInt32(node,12):0});
            }
            return new {thread=tid,error=error,cycle=cycle,nodes=nodes};
        } finally {Marshal.FreeHGlobal(buffer);CloseThreadWaitChainSession(session);}
    }
}
'@
$process.Threads | Select-Object -First 32 | ForEach-Object { [AtlasWaitEvidence]::Read([uint32]$_.Id) } | ConvertTo-Json -Depth 5
