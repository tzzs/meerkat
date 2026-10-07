param([string]$Title = "Pulse", [string]$Out = "E:\workspaces\Pulse\shots\window.png", [int]$Width = 1160, [int]$Height = 800)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Cap2 {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string n);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
}
"@
$h = [Cap2]::FindWindow($null, $Title)
if ($h -eq [IntPtr]::Zero) { Write-Output "window not found"; exit 1 }
$bmp = New-Object System.Drawing.Bitmap($Width, $Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::Fuchsia)
$hdc = $g.GetHdc()
$ok = [Cap2]::PrintWindow($h, $hdc, 2)
$g.ReleaseHdc($hdc)
$g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "printwindow=$ok vis=$([Cap2]::IsWindowVisible($h)) hwnd=$h -> $Out"
