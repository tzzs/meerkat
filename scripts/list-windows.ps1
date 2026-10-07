param([int]$TargetPid = 0)
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
public class WinEnum {
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc l, IntPtr p);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public static List<string> Collect(int target) {
    var list = new List<string>();
    EnumWindows((h, p) => {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (target != 0 && pid != (uint)target) return true;
      var sb = new StringBuilder(256); GetWindowText(h, sb, 256);
      var cls = new StringBuilder(128); GetClassName(h, cls, 128);
      RECT r; GetWindowRect(h, out r);
      list.Add(string.Format("pid={0} vis={1} rect=({2},{3})-({4},{5}) class={6} title={7}",
        pid, IsWindowVisible(h), r.Left, r.Top, r.Right, r.Bottom, cls, sb));
      return true;
    }, IntPtr.Zero);
    return list;
  }
}
"@
$procs = Get-Process -Name pulse -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id
if (-not $procs) { Write-Output "no pulse process" }
foreach ($p in $procs) {
  Write-Output "--- pulse pid $p ---"
  [WinEnum]::Collect($p) | ForEach-Object { Write-Output $_ }
}
