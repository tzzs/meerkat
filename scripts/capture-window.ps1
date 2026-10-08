# Captures the running app window. PrintWindow with PW_RENDERFULLCONTENT is used rather than
# a screen grab because a fullscreen game can own the z-order, and because the window may be
# behind everything and still have to be inspected.
param(
  [string]$Title = "Pulse",
  [string]$Out = "E:\workspaces\Pulse\shots\window.png",
  [string]$Process = "pulse",
  [int]$Index = 0
)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
public class Cap {
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc l, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public static HashSet<uint> Pids = new HashSet<uint>();
  public static string Want;
  public static List<string> Found = new List<string>();
  public static IntPtr[] Hits = new IntPtr[0];
  public static bool Cb(IntPtr h, IntPtr p) {
    uint pid; GetWindowThreadProcessId(h, out pid);
    if (!Pids.Contains(pid)) return true;
    var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256);
    var title = sb.ToString();
    if (!IsWindowVisible(h)) return true;
    if (!title.Contains(Want)) return true;
    RECT r; GetWindowRect(h, out r);
    Found.Add(string.Format("hwnd={0} title={1} rect={2}x{3}", h, title, r.Right - r.Left, r.Bottom - r.Top));
    var list = new List<IntPtr>(Hits); list.Add(h); Hits = list.ToArray();
    return true;
  }
  // The callback is a C# lambda rather than a script delegate: a delegate handed to
  // EnumWindows from PowerShell can be collected while the enumeration is still running.
  public static void Scan() {
    Hits = new IntPtr[0]; Found.Clear();
    EnumWindows((h, p) => Cb(h, p), IntPtr.Zero);
  }
}
"@
$pids = @(Get-Process -Name $Process -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)
if (-not $pids) { Write-Output "no $Process process"; exit 1 }
foreach ($p in $pids) { [Cap]::Pids.Add([uint32]$p) | Out-Null }
[Cap]::Want = $Title
[Cap]::Scan()
[Cap]::Found | ForEach-Object { Write-Output $_ }
if ([Cap]::Hits.Length -le $Index) { Write-Output "no matching window"; exit 1 }
$h = [Cap]::Hits[$Index]
$r = New-Object 'Cap+RECT'
[Cap]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.Right - $r.Left; $ht = $r.Bottom - $r.Top
$bmp = New-Object System.Drawing.Bitmap($w, $ht)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::Fuchsia)
$hdc = $g.GetHdc()
$ok = [Cap]::PrintWindow($h, $hdc, 2)
$g.ReleaseHdc($hdc); $g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "printwindow=$ok ${w}x${ht} -> $Out"
