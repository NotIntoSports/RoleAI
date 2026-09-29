# 一键复现 Criterion 基准（lane-C C34）。
# 用法：powershell -File scripts\run-benchmarks.ps1 [-Filter <名称过滤>] [-SkipMaterials] [-SkipAudio]
# 结果：终端输出 + target/criterion/ 下的明细（含历史基线对比）。
param(
    [string]$Filter = "",
    [switch]$SkipMaterials,
    [switch]$SkipAudio
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot

Write-Host "=== 环境信息 ===" -ForegroundColor Cyan
$cpu = (Get-CimInstance Win32_Processor).Name
$memory = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1)
$os = (Get-CimInstance Win32_OperatingSystem).Caption
$osVersion = (Get-CimInstance Win32_OperatingSystem).Version
$rustc = (& rustc --version)
Write-Host "CPU:     $cpu"
Write-Host "内存:    $memory GB"
Write-Host "系统:    $os ($osVersion)"
Write-Host "Rust:    $rustc"
Write-Host "日期:    $((Get-Date).ToString('yyyy-MM-dd HH:mm zzz'))"
Write-Host ""

Set-Location (Join-Path $repoRoot "src-tauri")

$benchArgs = @()
if ($Filter -ne "") { $benchArgs += $Filter }
if ($SkipAudio -and -not $SkipMaterials) { $benchArgs += "--bench"; $benchArgs += "materials" }
if ($SkipMaterials -and -not $SkipAudio) { $benchArgs += "--bench"; $benchArgs += "audio_hotpath" }

Write-Host "=== cargo bench $($benchArgs -join ' ') ===" -ForegroundColor Cyan
cargo bench @benchArgs
if ($LASTEXITCODE -ne 0) { throw "cargo bench 失败，退出码 $LASTEXITCODE" }

Write-Host ""
Write-Host "=== 完成 ===" -ForegroundColor Green
Write-Host "明细与历史对比：src-tauri\target\criterion\（报告 index.html 可用浏览器打开）"
