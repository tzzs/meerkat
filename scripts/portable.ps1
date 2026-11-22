# Builds the portable archive from an already-compiled release binary.
#
#   powershell -NoProfile -File scripts/portable.ps1
#
# Tauri has no zip target for Windows, and the GNU build's exe needs exactly one non-system
# DLL beside it, so "portable" is those two files in a folder. Nothing is installed and nothing
# is registered: the app writes its data to %APPDATA%\com.tzz.meerkat on first run like any other
# build.
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$release = Join-Path $root "src-tauri/target/release"
$out = Join-Path $root "dist-portable"

$conf = Get-Content (Join-Path $root "src-tauri/tauri.conf.json") -Raw | ConvertFrom-Json
$version = $conf.version

$exe = Join-Path $release "meerkat.exe"
if (-not (Test-Path $exe)) { throw "missing $exe; run pnpm tauri build first" }
# The GNU build leaves WebView2Loader.dll beside the exe; the MSVC build links it in. Include
# it when it exists, so one archive step works with either toolchain.
$files = @($exe)
$loader = Join-Path $release "WebView2Loader.dll"
if (Test-Path $loader) { $files += $loader }

$stage = Join-Path $out "Meerkat"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage | Out-Null
Copy-Item $files -Destination $stage

$zip = Join-Path $out "Meerkat-$version-portable-x64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
New-Item -ItemType Directory -Path $out -Force | Out-Null
Compress-Archive -Path $stage -DestinationPath $zip
Remove-Item $stage -Recurse -Force

$bytes = (Get-Item $zip).Length
Write-Output ("portable: {0} ({1:N2} MiB)" -f (Split-Path $zip -Leaf), ($bytes / 1MB))

# Read the archive back: an installer that packs the wrong files still builds happily, so the
# only proof that matters is the entry list.
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::OpenRead($zip)
$archive.Entries | ForEach-Object { Write-Output ("  {0}  {1:N0} bytes" -f $_.FullName, $_.Length) }
$archive.Dispose()
