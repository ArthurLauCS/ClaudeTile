# 给磁贴窗口发一次左键单击（客户区物理像素坐标），不动真实鼠标；也可以只发移动（-Move）看悬停效果
param([int]$ProcessId, [int]$X, [int]$Y, [switch]$Move)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class C {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, System.Text.StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
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
$h = [C]::Find([uint32]$ProcessId)
$lp = [IntPtr](($Y -shl 16) -bor ($X -band 0xFFFF))
[C]::SendMessage($h, 0x0200, [IntPtr]0, $lp) | Out-Null
if (-not $Move) {
    [C]::SendMessage($h, 0x0201, [IntPtr]1, $lp) | Out-Null
    [C]::SendMessage($h, 0x0202, [IntPtr]0, $lp) | Out-Null
}
