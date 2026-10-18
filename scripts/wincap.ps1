# 截指定进程里某个窗口类的所有可见窗口（菜单 ClaudeTileMenu、提示 ClaudeTileToast），每个存一张 <Out>-<序号>.png；
# -Post 先给磁贴主窗口发一条消息（例如托盘右键：-Post 0x8001 -LParam 0x205）
param([int]$ProcessId, [string]$Class, [string]$Out, [int]$Post = 0, [int]$LParam = 0, [int]$WaitMs = 500)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class Wc {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, System.Text.StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    public static List<IntPtr> Find(uint want, string cls) {
        var list = new List<IntPtr>();
        EnumWindows((h, l) => {
            var sb = new System.Text.StringBuilder(64);
            GetClassName(h, sb, 64);
            uint pid; GetWindowThreadProcessId(h, out pid);
            if (sb.ToString() == cls && pid == want && IsWindowVisible(h)) list.Add(h);
            return true;
        }, IntPtr.Zero);
        return list;
    }
}
"@
[Wc]::SetProcessDPIAware() | Out-Null
if ($Post -ne 0) {
    $main = [Wc]::Find([uint32]$ProcessId, "ClaudeTileMain")[0]
    [Wc]::PostMessage($main, [uint32]$Post, [IntPtr]1, [IntPtr]$LParam) | Out-Null
    Start-Sleep -Milliseconds $WaitMs
}
$i = 0
foreach ($h in [Wc]::Find([uint32]$ProcessId, $Class)) {
    $r = New-Object Wc+RECT
    [Wc]::GetWindowRect($h, [ref]$r) | Out-Null
    $m = 16
    $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L + 2 * $m), ($r.B - $r.T + 2 * $m)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($r.L - $m, $r.T - $m, 0, 0, $bmp.Size)
    $bmp.Save("$Out-$i.png"); $g.Dispose(); $bmp.Dispose()
    "$i $($r.L),$($r.T) $($r.R - $r.L)x$($r.B - $r.T)"
    $i++
}
