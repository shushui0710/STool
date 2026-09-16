$ErrorActionPreference = "Continue"
# ---- 环境（Win + MSVC + rustup toolchain）----
$tb   = "C:\Users\86136\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin"
$crt  = "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Redist\MSVC\14.36.32532\x64\Microsoft.VC143.CRT"
$msvc = "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.37.32822"
$hw   = "C:\Program Files (x86)\Windows Kits\10"
$env:PATH = "$tb;$crt;C:\Users\86136\.cargo\bin;" + $env:PATH
$env:LIB = "$msvc\lib\x64;$msvc\ATLMFC\lib\x64;$hw\Lib\10.0.22621.0\um\x64;$hw\Lib\10.0.22621.0\ucrt\x64"
$env:INCLUDE = "$msvc\include;$msvc\ATLMFC\include;$hw\Include\10.0.22621.0\ucrt;$hw\Include\10.0.22621.0\um;$hw\Include\10.0.22621.0\shared;$hw\Include\10.0.22621.0\winrt"
$env:CARGO_INCREMENTAL = "0"

# ---- clippy 专用补丁（2026-09 实测）----
# `cargo-clippy` 会让 cargo 给构建脚本传一个**解析不到的** `RUSTC`，
# 于是 `embed-resource`（→ rustc_version 0.4.1）spawn rustc 直接
# NotFound panic（build script exit 101）。表现得很像「编译坏了」，其实只是环境。
# 对策：把 `RUSTC` 钉成绝对路径、并把 `RUSTC_WRAPPER` 置空。
# 已验证 lint 仍然生效（用故意犯 `clippy::let_and_return` 的 scratch crate 试过，
# 加了这两个变量之后 clippy 照样报错），不是「静默变绿」。
$env:RUSTC_WRAPPER = ""
$env:RUSTC = "$tb\rustc.exe"

# ---- 参数 ----
$crateDir = $args[0]
$tag      = $args[1]
$doBuild  = ($args[2] -eq "release")
$log      = "D:\STool\stool_gate_$tag.log"

"=== GATE $tag @ $(Get-Date -Format o) ===" | Out-File $log
"crate=$crateDir build=$doBuild" | Out-File $log -Append
Set-Location $crateDir

# 杀软（卡巴）会启发式拦截刚产出的二进制 → os error 5 / LNK1104。
# 不是代码问题：重跑即可（cargo 从缓存续上）。
function Run-Cargo([string]$exe, [string[]]$cargoArgs, [string]$step) {
  for ($i = 1; $i -le 4; $i++) {
    "--- [$step] attempt $i ---" | Out-File $log -Append
    & $exe @cargoArgs 2>&1 | Out-File $log -Append
    if ($LASTEXITCODE -eq 0) { "[$step] exit=0" | Out-File $log -Append; return 0 }
    if ($LASTEXITCODE -eq 5 -or $LASTEXITCODE -eq -1073741515) {
      "[$step] retryable exit=$LASTEXITCODE（多半是杀软拦截，重跑）" | Out-File $log -Append
      Start-Sleep -Seconds 4
      continue
    }
    "[$step] hard exit=$LASTEXITCODE" | Out-File $log -Append
    return $LASTEXITCODE
  }
  "[$step] GAVE UP" | Out-File $log -Append
  return 99
}

$c1 = Run-Cargo "$tb\cargo-clippy.exe" @("clippy","--all-targets","--","-D","warnings") "clippy"
$c2 = Run-Cargo "$tb\cargo.exe"        @("test","--tests") "test"
$c3 = -1
if ($doBuild) {
  $c3 = Run-Cargo "$tb\cargo.exe" @("build","--release") "build-release"
}
"SUMMARY clippy=$c1 test=$c2 build=$c3" | Out-File $log -Append
"=== ALL DONE ===" | Out-File $log -Append
