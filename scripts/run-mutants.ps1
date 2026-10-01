# 一键复现核心纯逻辑模块变异测试（lane-I I07）。
# 用法：
#   powershell -File scripts\run-mutants.ps1                     # 全部 5 个模块
#   powershell -File scripts\run-mutants.ps1 -Modules barge_in   # 单模块（支持多个：-Modules barge_in,metrics）
#   powershell -File scripts\run-mutants.ps1 -Jobs 2 -MutantsArgs --re,'barge_in.rs:96:'
# 结果：终端输出 + .codex-tmp\mutants-<时间戳>.log + src-tauri\mutants.out\（outcomes.json、
#       每个变异体的 log/ 与 diff/，.gitignore 不入库）。
# 说明：变异测试整体很慢（每变异体一次冷构建 + 全量 lib 测试），全部 5 个模块约数百个
#       变异体、并行 3 路需数小时；建议只在需要刷新报告时运行。
param(
    [string[]]$Modules = @(),   # 为空 = 手册指定的全部 5 个模块；元素可用短名（barge_in）或相对路径
    [int]$Jobs = 4,             # 并行变异体任务数
    [int]$TestThreads = 4,      # 每路测试的 RUST_TEST_THREADS；0 = 不限定（吞吐优先但满载下基线易抖红）
    [string]$OutDir = "",       # 结果目录；为空 = 默认 src-tauri\mutants.out（分段运行时指向 .codex-tmp 下独立目录）
    [string]$OnlyRegex = "",    # 只测名称匹配该正则的变异体（补测后定向复验存活变异体用）
    [string]$Shard = "",        # 分片运行（如 "1/2"）：长模块被外部中断时可分片重跑，结果按变异体合并
    [switch]$NoShuffle,         # 按源码顺序运行（模块成块完成，便于逐模块流水分析）
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$MutantsArgs = @() # 透传给 cargo mutants 的其余参数
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot

# 兼容 "-Modules a,b,c"（单 token 逗号串）与 "-Modules a -Modules b" 两种传法
$Modules = @($Modules | ForEach-Object { $_ -split "[, ]+" } | Where-Object { $_ })

# I07 手册指定的纯逻辑模块（无 IO/设备/真实时间依赖，变异结果可解释）
$KnownModules = @(
    "src/services/echo_guard.rs",
    "src/audio/segmenter.rs",
    "src/audio/barge_in.rs",
    "src/materials/chunk.rs",
)

if ($Modules.Count -eq 0) {
    $files = $KnownModules
} else {
    $files = foreach ($m in $Modules) {
        $hit = $KnownModules | Where-Object { $_ -like "*$m*" }
        if (-not $hit) { throw "未知模块 '$m'，可选：$($KnownModules -join ', ')" }
        $hit
    }
}

$fileArgs = @()
foreach ($f in $files) { $fileArgs += @("--file", $f) }

Write-Host "=== 环境信息 ===" -ForegroundColor Cyan
Write-Host "CPU:   $((Get-CimInstance Win32_Processor).Name)"
Write-Host "Rust:  $(& rustc --version)"
Write-Host "工具:  $(& cargo mutants --version)"
Write-Host "模块:  $($files -join ', ')"
Write-Host "并行:  $Jobs 路变异体$(if ($TestThreads -gt 0) { "，每路 RUST_TEST_THREADS=$TestThreads" } else { "，测试线程不限定" })"
Write-Host "日期:  $((Get-Date).ToString('yyyy-MM-dd HH:mm zzz'))"
# sccache（若已安装）：deps 在各副本间逐变异体重编是构建瓶颈（实测 132s/个），
# 缓存命中后只重编被变异的 lib 本体。
$sccache = Get-Command sccache -ErrorAction SilentlyContinue
if ($sccache) {
    $env:RUSTC_WRAPPER = $sccache.Source
    Write-Host "缓存:  sccache = $($env:RUSTC_WRAPPER)"
} else {
    Write-Host "缓存:  未检测到 sccache，跳过构建缓存"
}
Write-Host ""

$tmpDir = Join-Path $repoRoot ".codex-tmp"
New-Item -ItemType Directory -Force $tmpDir | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"

# tauri.conf.json 的 bundle.resources 引用了仓库根的文件（..\scripts\*.ps1、
# ..\native\AudioBridge\publish\AudioBridge.exe）。cargo-mutants 只复制 src-tauri，
# 副本里 ..\ 指向临时根，找不到这些文件时 build.rs 直接失败（实测）。
# 解法：把 TEMP/TMP 指到 .codex-tmp\mutants-temp，并按资源清单把外部文件镜像到那里，
# 使每个副本的 ..\ 都能解析到同内容的镜像。
$mutTemp = Join-Path $tmpDir "mutants-temp"
New-Item -ItemType Directory -Force $mutTemp | Out-Null
$conf = Get-Content (Join-Path $repoRoot "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json
foreach ($r in $conf.bundle.resources) {
    if ($r -like "../*") {
        $rel = $r -replace "^(\.\./)+", ""
        $srcPath = Join-Path $repoRoot $rel
        $dstPath = Join-Path $mutTemp $rel
        if (Test-Path $srcPath) {
            New-Item -ItemType Directory -Force (Split-Path -Parent $dstPath) | Out-Null
            Copy-Item $srcPath $dstPath -Force
        } else {
            Write-Warning "外部资源不存在，副本构建可能失败：$srcPath"
        }
    }
}
# 清理上次中断泄漏的副本目录（仅在本脚本串行使用的前提下安全）
Get-ChildItem $mutTemp -Directory -Filter ".tmp*" -ErrorAction SilentlyContinue |
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
$env:TEMP = $mutTemp
$env:TMP = $mutTemp

# 测试线程数：默认 4（M 线实测满载下 busy-db IPC 测试需要 4 线程才稳定；
# 变异运行全程满载，不钉线程会让基线/变异体测试偶发红，基线红会中止整轮）。
if ($TestThreads -gt 0) { $env:RUST_TEST_THREADS = "$TestThreads" }
# MSVC 工具链输出固定英文（VSLANG=1033）：中文链接器诊断会以 GBK 混入变异体日志，
# cargo-mutants 汇总读取日志时按 UTF-8 解析会直接 panic（实测）。
$env:VSLANG = "1033"
# 单变异体测试超时下限 240s：自动超时基于基线时间乘数，多路并行负载会拉长全部测试，
# 下限放宽可避免把"负载变慢"误判成 Timeout（Timeout 计入杀死分数，会虚高）。
$timeoutArgs = @("--minimum-test-timeout", "240")

Set-Location (Join-Path $repoRoot "src-tauri")

# PS 5.1 注意：`native 2>&1 | Tee-Object` 会把 stderr 行升级成 ErrorRecord，配合
# $ErrorActionPreference='Stop' 会在 cargo-mutants 第一条进度输出时中止整个脚本。
# 因此用 Start-Process 把 stdout/stderr 分离写入文件，结束后再回显。
$outLog = Join-Path $tmpDir "mutants-$stamp.log"
$errLog = Join-Path $tmpDir "mutants-$stamp.err.log"
$argList = @("mutants") + $fileArgs + @("--jobs", "$Jobs")
if ($OutDir -ne "") { $argList += @("-o", $OutDir) }
if ($OnlyRegex -ne "") { $argList += @("--re", $OnlyRegex) }
if ($NoShuffle) { $argList += "--no-shuffle" }
if ($Shard -ne "") { $argList += @("--shard", $Shard) }
$argList += $timeoutArgs + $MutantsArgs
Write-Host "=== cargo mutants $($argList -join ' ') ===" -ForegroundColor Cyan
Write-Host "stdout: $outLog"
Write-Host "stderr: $errLog"
Write-Host "（进度请轮询日志文件：Get-Content -Wait $errLog）"
Write-Host ""

$proc = Start-Process -FilePath "cargo" -ArgumentList $argList `
    -WorkingDirectory (Join-Path $repoRoot "src-tauri") `
    -NoNewWindow -Wait -PassThru `
    -RedirectStandardOutput $outLog -RedirectStandardError $errLog

Write-Host ""
Write-Host "=== stderr 尾部 ===" -ForegroundColor Cyan
Get-Content $errLog -Tail 30 | Write-Host
if ($proc.ExitCode -ne 0) { throw "cargo mutants 失败，退出码 $($proc.ExitCode)（完整日志见上方两个文件）" }

Write-Host ""
Write-Host "=== 完成 ===" -ForegroundColor Green
Write-Host "明细：src-tauri\mutants.out\outcomes.json（log\ 每变异体日志、diff\ 每变异体差异，missed.txt 即存活变异体清单）"
Write-Host "补测后可用 -MutantsArgs --re,'<文件>:<行>:' 只重跑指定变异体"
