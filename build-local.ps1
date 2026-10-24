param(
    [switch]$SkipSmokeTest
)

$ErrorActionPreference = "Stop"

# 本地快速打包入口：
# - 只服务当前开发者本机，不参与版本控制
# - 复用公开仓库的标准构建链路，避免再手动敲一串安装/构建命令
$repoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $repoRoot

if (-not (Test-Path ".private-requirements.txt")) {
    throw "Missing .private-requirements.txt. Create the file manually and fill in your private dependency source first."
}

function Invoke-CheckedPython {
    param([string[]]$CommandArguments)
    & python @CommandArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Python command failed with exit code ${LASTEXITCODE}: python $($CommandArguments -join ' ')"
    }
}

Invoke-CheckedPython -CommandArguments @('-m', 'pip', 'install', '--upgrade', 'pip')
Invoke-CheckedPython -CommandArguments @('-m', 'pip', 'install', '-r', 'requirements.txt')
Invoke-CheckedPython -CommandArguments @('-m', 'pip', 'install', '-r', '.private-requirements.txt')
Invoke-CheckedPython -CommandArguments @('-m', 'pip', 'install', 'pyinstaller')
Invoke-CheckedPython -CommandArguments @('build.py')

if (-not $SkipSmokeTest) {
    $smokeExecutable = (Resolve-Path './dist/WirelessDeviceBatteryMonitor.exe').Path
    $smokeProcess = Start-Process -FilePath $smokeExecutable -ArgumentList @('--smoke-test') -Wait -PassThru -WindowStyle Hidden
    if ($smokeProcess.ExitCode -ne 0) {
        throw "Frozen executable smoke test failed with exit code $($smokeProcess.ExitCode)"
    }
}

Write-Host "Local build completed."
