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
$installMetadataFile = Join-Path $programDataDir 'install-metadata.json'
$desktop = [Environment]::GetFolderPath('DesktopDirectory')
$shortcutPath = Join-Path $desktop 'Claude Code 模型切换器.lnk'
$claudeUserConfig = Join-Path $env:USERPROFILE '.claude.json'
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)

function Write-Utf8TextAtomically {
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string]$Contents
    )
    $temporaryPath = "$Path.tmp-$PID-$([Guid]::NewGuid().ToString('N'))"
    try {
        [System.IO.File]::WriteAllText($temporaryPath, $Contents, $utf8NoBom)
        Move-Item -LiteralPath $temporaryPath -Destination $Path -Force
    }
    finally {
        Remove-Item -LiteralPath $temporaryPath -Force -ErrorAction SilentlyContinue
    }
}

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

if (Test-Path -LiteralPath $installMetadataFile -PathType Leaf) {
    try {
        $metadata = [System.IO.File]::ReadAllText($installMetadataFile) |
            ConvertFrom-Json
        $claudeSettings = [string]$metadata.claude_settings
        if (-not [string]::IsNullOrWhiteSpace($claudeSettings) -and
            (Test-Path -LiteralPath $claudeSettings -PathType Leaf)) {
            $settings = [System.IO.File]::ReadAllText($claudeSettings) |
                ConvertFrom-Json
            if ($null -ne $settings.PSObject.Properties['env'] -and
                $null -ne $settings.env) {
                Copy-Item `
                    -LiteralPath $claudeSettings `
                    -Destination "$claudeSettings.backup-$(Get-Date -Format 'yyyyMMdd-HHmmss')" `
                    -Force
                foreach ($previous in @($metadata.previous_env)) {
                    $current = $settings.env.PSObject.Properties[[string]$previous.name]
                    $installed = @($metadata.installed_env) |
                        Where-Object { $_.name -eq $previous.name } |
                        Select-Object -First 1
                    if ($null -eq $current -or $null -eq $installed -or
                        [string]$current.Value -cne [string]$installed.value) {
                        continue
                    }
                    if ([bool]$previous.existed) {
                        $current.Value = $previous.value
                    }
                    else {
                        $settings.env.PSObject.Properties.Remove([string]$previous.name)
                    }
                }
                Write-Utf8TextAtomically `
                    -Path $claudeSettings `
                    -Contents ($settings | ConvertTo-Json -Depth 100)
            }
        }
    }
    catch {
        Write-Warning "无法恢复 Claude 环境设置：$($_.Exception.Message)"
    }
}

if (Test-Path -LiteralPath $claudeUserConfig -PathType Leaf) {
    try {
        $claudeUser = [System.IO.File]::ReadAllText($claudeUserConfig) |
            ConvertFrom-Json
        if ($null -ne $claudeUser.PSObject.Properties['mcpServers'] -and
            $null -ne $claudeUser.mcpServers -and
            $null -ne $claudeUser.mcpServers.PSObject.Properties['gemini-image']) {
            Copy-Item `
                -LiteralPath $claudeUserConfig `
                -Destination "$claudeUserConfig.backup-$(Get-Date -Format 'yyyyMMdd-HHmmss')" `
                -Force
            $claudeUser.mcpServers.PSObject.Properties.Remove('gemini-image')
            Write-Utf8TextAtomically `
                -Path $claudeUserConfig `
                -Contents ($claudeUser | ConvertTo-Json -Depth 100)
        }
    }
    catch {
        Write-Warning "无法移除 Gemini 生图工具配置：$($_.Exception.Message)"
    }
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
Write-Host 'Claude 环境变量已按安装前快照恢复；卸载后用户自行修改的值保持不变。'
Write-Host '模型配置文件未自动删除。'
