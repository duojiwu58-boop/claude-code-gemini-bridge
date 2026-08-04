param(
    [switch]$RemoveConfiguration,
    [switch]$KeepProgramFiles
)

$ErrorActionPreference = 'Stop'

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator
    )
}

if (-not (Test-IsAdministrator)) {
    if ($PSBoundParameters.Count -gt 0) {
        throw '使用命令行参数卸载时，请先打开“管理员 PowerShell”。'
    }
    $elevationArguments = (
        '-NoProfile -ExecutionPolicy Bypass -File "{0}"' -f $PSCommandPath
    )
    $elevated = Start-Process `
        -FilePath 'powershell.exe' `
        -ArgumentList $elevationArguments `
        -Verb RunAs `
        -Wait `
        -PassThru
    exit $elevated.ExitCode
}

$serviceName = 'ClaudeCodeBridge'
$installDir = Join-Path $env:ProgramFiles 'ClaudeCodeBridge'
$programDataDir = Join-Path $env:ProgramData 'ClaudeCodeBridge'
$desktop = [Environment]::GetFolderPath('DesktopDirectory')
$shortcutPath = Join-Path $desktop 'Claude Code 模型切换器.lnk'

$service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($null -ne $service) {
    if ($service.Status -ne 'Stopped') {
        Stop-Service -Name $serviceName -Force
        $service.WaitForStatus(
            [System.ServiceProcess.ServiceControllerStatus]::Stopped,
            [TimeSpan]::FromSeconds(30)
        )
    }
    & sc.exe delete $serviceName | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "删除服务失败，sc.exe 返回 $LASTEXITCODE。"
    }
}

if (Test-Path -LiteralPath $shortcutPath -PathType Leaf) {
    Remove-Item -LiteralPath $shortcutPath -Force
}

$expectedInstallDir = [System.IO.Path]::GetFullPath(
    (Join-Path $env:ProgramFiles 'ClaudeCodeBridge')
)
if (-not $KeepProgramFiles -and
    (Test-Path -LiteralPath $installDir -PathType Container)) {
    $resolvedInstallDir = [System.IO.Path]::GetFullPath($installDir)
    if (-not [string]::Equals(
        $resolvedInstallDir,
        $expectedInstallDir,
        [StringComparison]::OrdinalIgnoreCase
    )) {
        throw "拒绝删除意外目录：$resolvedInstallDir"
    }
    Remove-Item -LiteralPath $resolvedInstallDir -Recurse -Force
}

if ($RemoveConfiguration -and (Test-Path -LiteralPath $programDataDir -PathType Container)) {
    $expectedDataDir = [System.IO.Path]::GetFullPath(
        (Join-Path $env:ProgramData 'ClaudeCodeBridge')
    )
    $resolvedDataDir = [System.IO.Path]::GetFullPath($programDataDir)
    if (-not [string]::Equals(
        $resolvedDataDir,
        $expectedDataDir,
        [StringComparison]::OrdinalIgnoreCase
    )) {
        throw "拒绝删除意外目录：$resolvedDataDir"
    }
    Remove-Item -LiteralPath $resolvedDataDir -Recurse -Force
    Write-Host '服务数据、API Key 和日志已经删除。'
}
else {
    Write-Host "服务数据已保留：$programDataDir"
}

Write-Host 'Claude Code Bridge 服务已经卸载。'
Write-Host '为避免破坏其他设置，Claude 的 settings.json 和模型配置文件未自动删除。'
