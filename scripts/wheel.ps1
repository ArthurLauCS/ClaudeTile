# 给磁贴窗口发滚轮消息（-Delta 负数往下滚），X / Y 是客户区物理像素坐标，不动真实鼠标
param([int]$ProcessId, [int]$X, [int]$Y, [int]$Delta = -120)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Wh {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, System.Text.StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    public static IntPtr Find(uint want) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            var sb = new System.Text.StringBuilder(64);
            GetClassName(h, sb, 64);
            uint pid; GetWindowThreadProcessId(h, out pid);
            if (sb.ToString() == "ClaudeTileMain" && pid == want) { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
"@
$h = [Wh]::Find([uint32]$ProcessId)
$pt = New-Object Wh+POINT
$pt.X = $X; $pt.Y = $Y
[Wh]::ClientToScreen($h, [ref]$pt) | Out-Null
$lp = [IntPtr](($pt.Y -shl 16) -bor ($pt.X -band 0xFFFF))
$wp = [IntPtr](($Delta -band 0xFFFF) -shl 16)
[Wh]::SendMessage($h, 0x020A, $wp, $lp) | Out-Null
