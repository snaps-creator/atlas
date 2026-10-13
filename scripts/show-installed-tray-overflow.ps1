$ErrorActionPreference='Stop'
if($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows'){throw 'Disposable GitHub Windows runner required'}
Add-Type -Path (Join-Path $PSScriptRoot 'installed-shell.cs')
$shell=[AtlasInstalledShell]::ShellPid();[AtlasInstalledShell]::VerifyShell($shell)
if([AtlasInstalledShell]::Integrity($shell) -ne 8192){throw 'Notification overflow requires the ordinary fixture shell'}
if([AtlasInstalledShell]::Integrity([uint32]$PID) -ne 8192){throw 'Notification overflow input requires an ordinary medium user'}
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes
$condition=[Windows.Automation.AndCondition]::new(
    [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,[int]$shell),
    [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ClassNameProperty,'Shell_TrayWnd'))
$taskbar=[Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children,$condition)
if(-not $taskbar){throw 'Verified taskbar automation element missing'}
$buttons=$taskbar.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Button))
$chevrons=@($buttons | Where-Object {$_.Current.Name -in 'Notification Chevron','Show hidden icons'})
if($chevrons.Count -eq 0){
    # The classic Windows2022 notification area can omit its Win32 chevron
    # from UIA. Click only the unique stock button directly under TrayNotifyWnd.
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class AtlasClassicOverflow {
    [StructLayout(LayoutKind.Sequential)] struct Rect {public int left,top,right,bottom;}
    [StructLayout(LayoutKind.Sequential)] struct Point {public int x,y;}
    [StructLayout(LayoutKind.Sequential)] struct Mouse {public int x,y;public uint data,flags,time;public UIntPtr extra;}
    [StructLayout(LayoutKind.Sequential)] struct Input {public uint type;public Mouse mouse;}
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr FindWindow(string name,string title);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr FindWindowEx(IntPtr parent,IntPtr after,string name,string title);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window,out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] static extern bool IsWindowEnabled(IntPtr window);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr window,out Rect rect);
    [DllImport("user32.dll")] static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x,int y);
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point point);
    [DllImport("user32.dll")] static extern uint SendInput(uint count,Input[] input,int size);
    [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    public static void Click(uint shell) {
        var previous=SetThreadDpiAwarenessContext(new IntPtr(-4));if(previous==IntPtr.Zero)throw new Exception("Chevron physical coordinates unavailable");
        Point cursor;bool saved=GetCursorPos(out cursor);
        try {
            var taskbar=FindWindow("Shell_TrayWnd",null);uint owner;GetWindowThreadProcessId(taskbar,out owner);if(owner!=shell)throw new Exception("Taskbar owner changed");
            var area=FindWindowEx(taskbar,IntPtr.Zero,"TrayNotifyWnd",null);
            var button=FindWindowEx(area,IntPtr.Zero,"Button",null);
            if(area==IntPtr.Zero || button==IntPtr.Zero || FindWindowEx(area,button,"Button",null)!=IntPtr.Zero)throw new Exception("Unique classic notification chevron unavailable");
            GetWindowThreadProcessId(button,out owner);Rect rect;
            if(owner!=shell || !IsWindowVisible(button) || !IsWindowEnabled(button) || !GetWindowRect(button,out rect) || rect.right<=rect.left || rect.bottom<=rect.top)throw new Exception("Classic chevron is not a live enabled shell button");
            var point=new Point{x=rect.left+(rect.right-rect.left)/2,y=rect.top+(rect.bottom-rect.top)/2};
            if(!saved || WindowFromPoint(point)!=button || !SetCursorPos(point.x,point.y))throw new Exception("Classic chevron input point is obstructed");
            Point actual;if(!GetCursorPos(out actual) || actual.x!=point.x || actual.y!=point.y)throw new Exception("Chevron cursor read-back failed");
            var inputs=new[]{new Input{mouse=new Mouse{flags=2}},new Input{mouse=new Mouse{flags=4}}};
            if(SendInput(2,inputs,Marshal.SizeOf(typeof(Input)))!=2)throw new Exception("Classic chevron mouse input failed");
        }finally{if(saved)SetCursorPos(cursor.x,cursor.y);SetThreadDpiAwarenessContext(previous);}
    }
}
'@
    [AtlasClassicOverflow]::Click($shell)
} else {
    if($chevrons.Count -ne 1 -or -not $chevrons[0].Current.IsEnabled){throw 'Notification overflow button is ambiguous or disabled'}
    $pattern=$null
    if(-not $chevrons[0].TryGetCurrentPattern([Windows.Automation.InvokePattern]::Pattern,[ref]$pattern)){throw 'Stock notification overflow action unavailable'}
    $pattern.Invoke()
}
$popupCondition=[Windows.Automation.AndCondition]::new(
    [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,[int]$shell),
    [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ClassNameProperty,'NotifyIconOverflowWindow'))
$deadline=[DateTime]::UtcNow.AddSeconds(3)
do {
    $popup=[Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children,$popupCondition)
    if($popup -and -not $popup.Current.IsOffscreen){Write-Output 'PASS: stock notification overflow is visible';return}
    Start-Sleep -Milliseconds 50
} while([DateTime]::UtcNow -lt $deadline)
throw 'Stock notification overflow did not become visible'
