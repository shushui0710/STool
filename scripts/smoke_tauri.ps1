# 壳 exe 冒烟：启动 -> 存活 N 秒 -> 收掉。防「静默白窗」（custom-protocol 没开之类的坑）。
# 用法： powershell -File scripts\smoke_tauri.ps1 [存活秒数] [exe路径]
param(
  [int]$Seconds = 10,
  [string]$Exe = "D:\STool\stool-tauri\src-tauri\target\release\stool-tauri.exe"
)
$ErrorActionPreference = "Continue"
$log = "D:\STool\smoke_tauri.log"
"=== SMOKE $Exe @ $(Get-Date -Format o) ===" | Out-File $log
if (-not (Test-Path $Exe)) { "MISSING exe: $Exe" | Out-File $log -Append; exit 2 }
$fi = Get-Item $Exe
"exe mtime = $($fi.LastWriteTime.ToString('o'))  size = $($fi.Length)" | Out-File $log -Append

$p = Start-Process -FilePath $Exe -WorkingDirectory (Split-Path $Exe) -PassThru
"started pid = $($p.Id)" | Out-File $log -Append
Start-Sleep -Seconds $Seconds
$alive = -not $p.HasExited
"alive after ${Seconds}s = $alive" | Out-File $log -Append
if ($alive) {
  $m = (Get-Process -Id $p.Id -ErrorAction SilentlyContinue)
  if ($m) {
    "workingSet = $([math]::Round($m.WorkingSet64/1MB,1)) MB  threads = $($m.Threads.Count)" | Out-File $log -Append
  }
} else {
  "EXITED EARLY exitCode = $($p.ExitCode)" | Out-File $log -Append
}
if ($alive) { Stop-Process -Id $p.Id -Force; Start-Sleep -Seconds 1 }
$left = @(Get-Process -Name "stool-tauri" -ErrorAction SilentlyContinue)
"leftover stool-tauri processes = $($left.Count)" | Out-File $log -Append
if ($alive) { "RESULT = PASS（存活 ${Seconds}s 不崩，已收掉）" | Out-File $log -Append; exit 0 }
"RESULT = FAIL（提前退出）" | Out-File $log -Append
exit 1
