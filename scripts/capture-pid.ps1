param([string]$ProcessName = "meerkat", [string]$Out = (Join-Path (Split-Path $PSScriptRoot) "shots\win.png"))
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
using System.Collections.Generic;
public class Cap3 {
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc l, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public static List<IntPtr> Found = new List<IntPtr>();
  public static int TargetPid;
  public static bool Cb(IntPtr h, IntPtr p) {
    uint pid; GetWindowThreadProcessId(h, out pid);
    if (pid != (uint)TargetPid) return true;
    var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256);
    if (sb.Length > 0 && IsWindowVisible(h)) Found.Add(h);
    return true;
  }
  public static List<IntPtr> Collect(int pid) { Found.Clear(); TargetPid = pid; EnumWindows(Cb, IntPtr.Zero); return Found; }
}
"@
$procs = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id
if (-not $procs) { Write-Output "no $ProcessName process"; exit 1 }
foreach ($procId in $procs) {
  $list = [Cap3]::Collect($procId)
  foreach ($h in $list) {
    $r = New-Object 'Cap3+RECT'
    [Cap3]::GetWindowRect($h, [ref]$r) | Out-Null
    $w = $r.Right - $r.Left; $ht = $r.Bottom - $r.Top
    if ($w -lt 100 -or $ht -lt 100) { continue }
    $bmp = New-Object System.Drawing.Bitmap($w, $ht)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear([System.Drawing.Color]::Fuchsia)
    $hdc = $g.GetHdc()
    $ok = [Cap3]::PrintWindow($h, $hdc, 2)
    $g.ReleaseHdc($hdc); $g.Dispose()
    $file = $Out -replace '\.png$', "-$($w)x$($ht).png"
    $bmp.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Output "hwnd=$h ${w}x${ht} printwindow=$ok -> $file"
  }
}
