param(
    [switch]$RemoveConfiguration,
    [switch]$KeepProgramFiles,
    [string]$ElevationUserProfile,
    [string]$ElevationUserDesktop
)

$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# PowerShell 2.0 (Windows 7 SP1) compatible helpers.
# ---------------------------------------------------------------------------

function Test-StringNullOrWhitespace {
    param([string]$Value)
    if ([string]::IsNullOrEmpty($Value)) {
        return $true
    }
    return $Value.Trim().Length -eq 0
}

function ConvertFrom-JsonCompat {
    param([Parameter(Mandatory)][string]$InputObject)
    if ($PSVersionTable.ContainsKey('PSEdition') -and
        $PSVersionTable['PSEdition'] -eq 'Core') {
        return Microsoft.PowerShell.Utility\ConvertFrom-Json `
            -InputObject $InputObject `
            -AsHashtable
    }
    Add-Type -AssemblyName System.Web.Extensions -ErrorAction SilentlyContinue
    $serializer = New-Object System.Web.Script.Serialization.JavaScriptSerializer
    $serializer.MaxJsonLength = [int]::MaxValue
    return $serializer.DeserializeObject($InputObject)
}

function ConvertTo-JsonSafeValue {
    param($Value)
    if ($null -eq $Value) {
        return $null
    }
    $typeName = $Value.GetType().FullName
    if ($typeName -eq 'System.Management.Automation.PSCustomObject' -or
        $typeName -eq 'System.Management.Automation.PSObject') {
        $result = @{}
        foreach ($prop in $Value.PSObject.Properties) {
            $result[$prop.Name] = ConvertTo-JsonSafeValue ($prop.Value)
        }
        return $result
    }
    if ($typeName -eq 'System.Collections.Hashtable') {
        $result = @{}
        foreach ($key in $Value.Keys) {
            $result[[string]$key] = ConvertTo-JsonSafeValue ($Value[$key])
        }
        return $result
    }
    if ($Value -is [System.Collections.IDictionary]) {
        $result = @{}
        foreach ($key in $Value.Keys) {
            $result[[string]$key] = ConvertTo-JsonSafeValue ($Value[$key])
        }
        return $result
    }
    if ($Value -is [System.Array] -or $Value -is [System.Collections.ArrayList]) {
        $list = New-Object System.Collections.ArrayList
        foreach ($item in $Value) {
            [void]$list.Add((ConvertTo-JsonSafeValue $item))
        }
        # JavaScriptSerializer cannot handle an ArrayList nested inside an
        # IDictionary, so return a plain object[] instead.
        return ,($list.ToArray())
    }
    return $Value
}

function ConvertTo-JsonCompat {
    param($InputObject)
    if ($PSVersionTable.ContainsKey('PSEdition') -and
        $PSVersionTable['PSEdition'] -eq 'Core') {
        return Microsoft.PowerShell.Utility\ConvertTo-Json `
            -InputObject $InputObject `
            -Depth 100 `
            -Compress
    }
    Add-Type -AssemblyName System.Web.Extensions -ErrorAction SilentlyContinue
    $serializer = New-Object System.Web.Script.Serialization.JavaScriptSerializer
    $serializer.MaxJsonLength = [int]::MaxValue
    return $serializer.Serialize((ConvertTo-JsonSafeValue $InputObject))
}

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object -TypeName Security.Principal.WindowsPrincipal -ArgumentList @($identity)
    return $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator
    )
}

if (-not (Test-IsAdministrator)) {
    if ($PSBoundParameters.Count -gt 0) {
        throw '使用命令行参数卸载时，请先打开“管理员 PowerShell”。'
    }
    $unelevatedProfile = [System.IO.Path]::GetFullPath($env:USERPROFILE)
    $unelevatedDesktop = [Environment]::GetFolderPath('DesktopDirectory')
    $elevationArguments = (
        (
            '-NoProfile -ExecutionPolicy Bypass -File "{0}" ' +
            '-ElevationUserProfile "{1}" -ElevationUserDesktop "{2}"'
        ) -f
        $MyInvocation.MyCommand.Path,
        $unelevatedProfile,
        $unelevatedDesktop
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
$targetUserProfile = if (Test-StringNullOrWhitespace ($ElevationUserProfile)) {
    [System.IO.Path]::GetFullPath($env:USERPROFILE)
}
else {
    [System.IO.Path]::GetFullPath($ElevationUserProfile)
}
$desktop = if (Test-StringNullOrWhitespace ($ElevationUserDesktop)) {
    [Environment]::GetFolderPath('DesktopDirectory')
}
else {
    [System.IO.Path]::GetFullPath($ElevationUserDesktop)
}
$shortcutPath = Join-Path $desktop 'Claude Code 模型切换器.lnk'
$claudeUserConfig = Join-Path $targetUserProfile '.claude.json'
$utf8NoBom = New-Object -TypeName System.Text.UTF8Encoding -ArgumentList @($false)

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
        Move-Item -Path $temporaryPath -Destination $Path -Force
    }
    finally {
        Remove-Item -Path $temporaryPath -Force -ErrorAction SilentlyContinue
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

if ([System.IO.File]::Exists($shortcutPath)) {
    Remove-Item -Path $shortcutPath -Force
}

if ([System.IO.File]::Exists($installMetadataFile)) {
    try {
        $metadata = ConvertFrom-JsonCompat ([System.IO.File]::ReadAllText($installMetadataFile))
        $claudeSettings = [string]$metadata['claude_settings']
        if ($metadata -is [System.Collections.IDictionary] -and
            -not (Test-StringNullOrWhitespace ($claudeSettings)) -and
            [System.IO.File]::Exists($claudeSettings)) {
            $settings = ConvertFrom-JsonCompat ([System.IO.File]::ReadAllText($claudeSettings))
            if ($settings -is [System.Collections.IDictionary] -and
                $settings.ContainsKey('env') -and
                $null -ne $settings['env'] -and
                $settings['env'] -is [System.Collections.IDictionary]) {
                Copy-Item `
                    -Path $claudeSettings `
                    -Destination "$claudeSettings.backup-$(Get-Date -Format 'yyyyMMdd-HHmmss')" `
                    -Force
                foreach ($previous in @($metadata['previous_env'])) {
                    $previousName = [string]$previous['name']
                    $installed = $null
                    foreach ($candidate in @($metadata['installed_env'])) {
                        if ([string]$candidate['name'] -eq $previousName) {
                            $installed = $candidate
                            break
                        }
                    }
                    if (-not $settings['env'].ContainsKey($previousName) -or
                        $null -eq $installed) {
                        continue
                    }
                    if ([string]$settings['env'][$previousName] -cne [string]$installed['value']) {
                        continue
                    }
                    if ([bool]$previous['existed']) {
                        $settings['env'][$previousName] = $previous['value']
                    }
                    else {
                        [void]$settings['env'].Remove($previousName)
                    }
                }
                Write-Utf8TextAtomically `
                    -Path $claudeSettings `
                    -Contents (ConvertTo-JsonCompat ($settings))
            }
        }
    }
    catch {
        Write-Warning "无法恢复 Claude 环境设置：$($_.Exception.Message)"
    }
}

if ([System.IO.File]::Exists($claudeUserConfig)) {
    try {
        $claudeUser = ConvertFrom-JsonCompat ([System.IO.File]::ReadAllText($claudeUserConfig))
        if ($claudeUser -is [System.Collections.IDictionary] -and
            $claudeUser.ContainsKey('mcpServers') -and
            $null -ne $claudeUser['mcpServers'] -and
            $claudeUser['mcpServers'] -is [System.Collections.IDictionary] -and
            ($claudeUser['mcpServers'].ContainsKey('gemini-image') -or
             $claudeUser['mcpServers'].ContainsKey('gemini-computer'))) {
            Copy-Item `
                -Path $claudeUserConfig `
                -Destination "$claudeUserConfig.backup-$(Get-Date -Format 'yyyyMMdd-HHmmss')" `
                -Force
            [void]$claudeUser['mcpServers'].Remove('gemini-image')
            [void]$claudeUser['mcpServers'].Remove('gemini-computer')
            Write-Utf8TextAtomically `
                -Path $claudeUserConfig `
                -Contents (ConvertTo-JsonCompat ($claudeUser))
        }
    }
    catch {
        Write-Warning "无法移除 Gemini 生图或 Computer Use MCP 工具配置：$($_.Exception.Message)"
    }
}

$expectedInstallDir = [System.IO.Path]::GetFullPath(
    (Join-Path $env:ProgramFiles 'ClaudeCodeBridge')
)
if (-not $KeepProgramFiles -and
    [System.IO.Directory]::Exists($installDir)) {
    $resolvedInstallDir = [System.IO.Path]::GetFullPath($installDir)
    if (-not [string]::Equals(
        $resolvedInstallDir,
        $expectedInstallDir,
        [StringComparison]::OrdinalIgnoreCase
    )) {
        throw "拒绝删除意外目录：$resolvedInstallDir"
    }
    Remove-Item -Path $resolvedInstallDir -Recurse -Force
}

if ($RemoveConfiguration -and [System.IO.Directory]::Exists($programDataDir)) {
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
    Remove-Item -Path $resolvedDataDir -Recurse -Force
    Write-Host '服务数据、API Key 和日志已经删除。'
}
else {
    Write-Host "服务数据已保留：$programDataDir"
}

Write-Host 'Claude Code Bridge 服务已经卸载。'
Write-Host 'Claude 环境变量已按安装前快照恢复；卸载后用户自行修改的值保持不变。'
Write-Host '模型配置文件未自动删除。'
