# 截磁贴窗口（按窗口类名找，多截 24px 边距看阴影）到指定文件
param([string]$Out, [int]$ProcessId = 0)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class W {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, System.Text.StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    public static IntPtr Find(uint want) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            var sb = new System.Text.StringBuilder(64);
            GetClassName(h, sb, 64);
            uint pid; GetWindowThreadProcessId(h, out pid);
            if (sb.ToString() == "ClaudeTileMain" && (want == 0 || pid == want)) { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
"@
[W]::SetProcessDPIAware() | Out-Null
$h = [W]::Find([uint32]$ProcessId)
if ($h -eq [IntPtr]::Zero) { Write-Error "window not found"; exit 1 }
$r = New-Object W+RECT
[W]::GetWindowRect($h, [ref]$r) | Out-Null
$m = 24
$w = $r.R - $r.L + 2 * $m; $hh = $r.B - $r.T + 2 * $m
$bmp = New-Object System.Drawing.Bitmap $w, $hh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.L - $m, $r.T - $m, 0, 0, $bmp.Size)
$bmp.Save($Out); $g.Dispose(); $bmp.Dispose()
"$($r.L),$($r.T) $($r.R - $r.L)x$($r.B - $r.T)"
