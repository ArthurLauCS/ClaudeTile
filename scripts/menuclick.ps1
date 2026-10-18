# 给右键菜单窗口发按下 / 拖动 / 松开（客户区物理像素坐标），不动真实鼠标。-ToX 给了就从 X 拖到 ToX
param([int]$ProcessId, [int]$X, [int]$Y, [int]$ToX = -1)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Mc {
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
            if (sb.ToString() == "ClaudeTileMenu" && pid == want) { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
"@
$h = [Mc]::Find([uint32]$ProcessId)
function MakeLp([int]$x, [int]$y) { [IntPtr](($y -shl 16) -bor ($x -band 0xFFFF)) }
[Mc]::SendMessage($h, 0x0201, [IntPtr]1, (MakeLp $X $Y)) | Out-Null
if ($ToX -ge 0) {
    for ($i = 1; $i -le 8; $i++) { [Mc]::SendMessage($h, 0x0200, [IntPtr]1, (MakeLp ($X + ($ToX - $X) * $i / 8) $Y)) | Out-Null }
    $X = $ToX
}
[Mc]::SendMessage($h, 0x0202, [IntPtr]0, (MakeLp $X $Y)) | Out-Null
