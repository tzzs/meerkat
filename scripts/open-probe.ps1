$ErrorActionPreference = "Stop"
$chrome = "C:\Program Files\Google\Chrome\Application\chrome.exe"
if (-not (Test-Path $chrome)) { Write-Output "chrome not found"; exit 1 }
$profile = Join-Path $env:TEMP "meerkat-probe-profile"
$url = "http://localhost:1420/?tab=cpu"
Start-Process -FilePath $chrome -ArgumentList @(
  "--app=$url",
  "--user-data-dir=$profile",
  "--remote-debugging-port=9333",
  "--window-size=1240,880",
  "--window-position=120,80",
  "--no-first-run",
  "--no-default-browser-check"
) | Out-Null
Start-Sleep -Seconds 5
try {
  $targets = Invoke-RestMethod -Uri "http://127.0.0.1:9333/json/list" -TimeoutSec 5
  $targets | Where-Object { $_.type -eq "page" } | ForEach-Object { Write-Output "$($_.url) | $($_.title)" }
} catch {
  Write-Output "devtools endpoint not reachable: $($_.Exception.Message)"
}
