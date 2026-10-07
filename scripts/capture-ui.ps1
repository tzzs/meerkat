param([string]$Url = "http://localhost:1420/?tab=memory", [string]$Out = "E:\workspaces\Pulse\shots\tab.png", [int]$Wait = 6)
$edge = @("C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe", "C:\Program Files\Microsoft\Edge\Application\msedge.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $edge) { Write-Output "edge not found"; exit 1 }
$profile = Join-Path $env:TEMP "pulse-ui-profile"
$proc = Start-Process -FilePath $edge -ArgumentList @("--app=$Url", "--window-size=$Win,$Hit", "--window-position=140,90", "--user-data-dir=$profile", "--no-first-run") -PassThru
Start-Sleep -Seconds $Wait
# The renderer lives in a child msedge process; capture every window the tree owns.
$tree = @($proc.Id) + @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($proc.Id)" | Select-Object -ExpandProperty ProcessId)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
public class Grab {
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc l, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public static List<IntPtr> Found = new List<IntPtr>();
  public static HashSet<uint> Pids = new HashSet<uint>();
  public static string Want;
  public static bool Cb(IntPtr h, IntPtr p) {
    uint pid; GetWindowThreadProcessId(h, out pid);
    var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256);
    if (Pids.Contains(pid) && IsWindowVisible(h) && sb.ToString().Contains(Want)) Found.Add(h);
    return true;
  }
  public static List<IntPtr> Scan(uint[] pids, string title) {
    Pids = new HashSet<uint>(pids); Want = title; Found.Clear();
    EnumWindows(Cb, IntPtr.Zero);
    return Found;
  }
}
"@
$found = [Grab]::Scan([uint32[]]$tree, "Pulse")
if ($found.Count -eq 0) { Write-Output "no Pulse window (pids: $tree)"; Stop-Process -Id $proc.Id -Force; exit 1 }
$h = $found[0]
$r = New-Object 'Grab+RECT'
[Grab]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.Right - $r.Left; $ht = $r.Bottom - $r.Top
$bmp = New-Object System.Drawing.Bitmap($w, $ht)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::Fuchsia)
$hdc = $g.GetHdc()
$ok = [Grab]::PrintWindow($h, $hdc, 2)
$g.ReleaseHdc($hdc); $g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "captured=$ok ${w}x${ht} -> $Out"
Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
