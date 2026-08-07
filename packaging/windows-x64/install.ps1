param(
    [string]$ProxyUrl,
    [switch]$DirectConnection,
    [int]$Port = 18787,
    [ValidateSet('Prompt', 'Configure', 'Skip')]
    [string]$GeminiMode = 'Prompt',
    [string]$ApiKeyFile,
    [string]$ClaudeSettingsDir,
    [string]$ProviderConfigDir,
    [switch]$NonInteractive,
    [switch]$SkipShortcuts
)

$ErrorActionPreference = 'Stop'
$proxyUrlSpecified = $PSBoundParameters.ContainsKey('ProxyUrl')

if ($DirectConnection -and $proxyUrlSpecified) {
    throw '-DirectConnection 不能与 -ProxyUrl 同时使用。'
}

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator
    )
}

if (-not (Test-IsAdministrator)) {
    if ($PSBoundParameters.Count -gt 0) {
        throw '使用命令行参数安装时，请先打开“管理员 PowerShell”。'
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

function Copy-FileIfNeeded {
    param(
        [Parameter(Mandatory)]
        [string]$Source,
        [Parameter(Mandatory)]
        [string]$Destination
    )

    $sourcePath = [System.IO.Path]::GetFullPath($Source)
    $destinationPath = [System.IO.Path]::GetFullPath($Destination)
    if ([string]::Equals(
        $sourcePath,
        $destinationPath,
        [StringComparison]::OrdinalIgnoreCase
    )) {
        return
    }
    Copy-Item -LiteralPath $sourcePath -Destination $destinationPath -Force
}

function Get-WindowsProxyUrl {
    $internetSettings = Get-ItemProperty `
        -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' `
        -ErrorAction SilentlyContinue
    if ($null -eq $internetSettings -or
        $internetSettings.ProxyEnable -ne 1 -or
        [string]::IsNullOrWhiteSpace([string]$internetSettings.ProxyServer)) {
        return $null
    }

    $proxyServer = [string]$internetSettings.ProxyServer
    $candidates = @{}
    foreach ($part in ($proxyServer -split ';')) {
        $trimmed = $part.Trim()
        if ($trimmed -match '^(?<scheme>https?|socks)=(?<address>.+)$') {
            $candidates[$Matches.scheme.ToLowerInvariant()] = $Matches.address.Trim()
        }
        elseif (-not $candidates.ContainsKey('default')) {
            $candidates.default = $trimmed
        }
    }
    $detected = if ($candidates.ContainsKey('https')) {
        $candidates.https
    }
    elseif ($candidates.ContainsKey('http')) {
        $candidates.http
    }
    elseif ($candidates.ContainsKey('default')) {
        $candidates.default
    }
    else {
        $null
    }
    if ([string]::IsNullOrWhiteSpace($detected)) {
        return $null
    }
    if ($detected -notmatch '^[a-z][a-z0-9+.-]*://') {
        $detected = "http://$detected"
    }
    return $detected
}

$serviceName = 'ClaudeCodeBridge'
$serviceAccount = "NT SERVICE\$serviceName"
$displayName = 'Claude Code Multi-Model Bridge'
$packageDir = Split-Path -Parent $PSCommandPath
$packageExe = Join-Path $packageDir 'claude-bridge.exe'
$packageGui = Join-Path $packageDir 'ClaudeBridgeManager.exe'
$bridgeSettingsTemplate = Join-Path $packageDir 'claude-settings.bridge.json'
$installDir = Join-Path $env:ProgramFiles 'ClaudeCodeBridge'
$programDataDir = Join-Path $env:ProgramData 'ClaudeCodeBridge'
$serviceExe = Join-Path $installDir 'claude-bridge.exe'
$legacyServiceExe = Join-Path $installDir 'codex-gemini-bridge.exe'
$guiExe = Join-Path $installDir 'ClaudeBridgeManager.exe'
$scriptsDir = Join-Path $installDir 'scripts'
$logDir = Join-Path $programDataDir 'logs'
$keyFile = Join-Path $programDataDir 'gemini-api-key.toml'
$stateFile = Join-Path $programDataDir 'bridge-state.json'
$claudeDir = if ([string]::IsNullOrWhiteSpace($ClaudeSettingsDir)) {
    Join-Path $env:USERPROFILE '.claude'
}
else {
    [System.IO.Path]::GetFullPath($ClaudeSettingsDir)
}
$claudeSettings = Join-Path $claudeDir 'settings.json'
$claudeUserConfig = Join-Path (Split-Path -Parent $claudeDir) '.claude.json'
$picturesDir = [Environment]::GetFolderPath(
    [Environment+SpecialFolder]::MyPictures
)
if ([string]::IsNullOrWhiteSpace($picturesDir)) {
    $picturesDir = Join-Path (Split-Path -Parent $claudeDir) 'Pictures'
}
$imageDir = Join-Path $picturesDir 'ClaudeCodeBridge'
$providersDir = if ([string]::IsNullOrWhiteSpace($ProviderConfigDir)) {
    Join-Path $claudeDir 'bridge-providers'
}
else {
    [System.IO.Path]::GetFullPath($ProviderConfigDir)
}
$geminiProfile = Join-Path $providersDir 'gemini.json'
$serviceRegistry = "HKLM:\SYSTEM\CurrentControlSet\Services\$serviceName"
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
New-Item -ItemType Directory -Path $providersDir -Force | Out-Null

function Grant-ServicePathAccess {
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [Security.AccessControl.FileSystemRights]$Rights
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }
    $item = Get-Item -LiteralPath $Path
    $inheritance = if ($item.PSIsContainer) {
        [Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
        [Security.AccessControl.InheritanceFlags]::ObjectInherit
    }
    else {
        [Security.AccessControl.InheritanceFlags]::None
    }
    $acl = Get-Acl -LiteralPath $Path
    $rule = [Security.AccessControl.FileSystemAccessRule]::new(
        $serviceAccount,
        $Rights,
        $inheritance,
        [Security.AccessControl.PropagationFlags]::None,
        [Security.AccessControl.AccessControlType]::Allow
    )
    $acl.SetAccessRule($rule)
    Set-Acl -LiteralPath $Path -AclObject $acl
}

if ($Port -ne 18787) {
    throw '当前 GUI 发布版固定使用端口 18787。'
}

$requiredFiles = @(
    $packageExe
    $packageGui
    $bridgeSettingsTemplate
    (Join-Path $packageDir 'scripts\start-bridge.ps1')
    (Join-Path $packageDir 'scripts\stop-bridge.ps1')
)
foreach ($requiredFile in $requiredFiles) {
    if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
        throw "发布包不完整，缺少文件：$requiredFile"
    }
}

Write-Host ''
Write-Host 'Claude Code Bridge Windows 服务安装程序'
Write-Host ''

if ($GeminiMode -eq 'Prompt') {
    if ($NonInteractive) {
        throw '非交互安装必须明确指定 -GeminiMode Configure 或 Skip。'
    }
    $configureAnswer = Read-Host '是否配置 Google Gemini？不使用请直接回车 [y/N]'
    $GeminiMode = if ($configureAnswer -match '^(?i:y|yes)$') {
        'Configure'
    }
    else {
        'Skip'
    }
}

$configureGemini = $GeminiMode -eq 'Configure'
$apiKey = $null
if ($configureGemini) {
    Write-Host 'API Key 只会保存在本机受限文件中，不会写入服务注册表。'
    if (-not [string]::IsNullOrWhiteSpace($ApiKeyFile)) {
        if (-not (Test-Path -LiteralPath $ApiKeyFile -PathType Leaf)) {
            throw "找不到安装器提供的 API Key 临时文件：$ApiKeyFile"
        }
        try {
            $apiKey = [System.IO.File]::ReadAllText($ApiKeyFile).Trim()
        }
        finally {
            Remove-Item -LiteralPath $ApiKeyFile -Force -ErrorAction SilentlyContinue
        }
    }
    else {
        if ($NonInteractive) {
            throw '配置 Gemini 时必须提供 -ApiKeyFile。'
        }
        $secureApiKey = Read-Host '请输入 Google AI Studio Gemini API Key' -AsSecureString
        $keyPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secureApiKey)
        try {
            $apiKey = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($keyPointer)
        }
        finally {
            [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($keyPointer)
        }
    }
    if ([string]::IsNullOrWhiteSpace($apiKey)) {
        throw 'API Key 不能为空。'
    }
}

$previousServiceEnvironment = @()
if (Test-Path -LiteralPath $serviceRegistry) {
    $previousServiceEnvironment = @(
        (
            Get-ItemProperty `
                -LiteralPath $serviceRegistry `
                -ErrorAction SilentlyContinue
        ).Environment
    )
}

$persistedProxyKnown = $false
$persistedProxy = $null
if (Test-Path -LiteralPath $stateFile -PathType Leaf) {
    try {
        $previousState = Get-Content -LiteralPath $stateFile -Raw | ConvertFrom-Json
        if ($null -ne $previousState.PSObject.Properties['gemini_proxy']) {
            $persistedProxyKnown = $true
            $persistedProxy = [string]$previousState.gemini_proxy
        }
    }
    catch {
        $persistedProxyKnown = $false
    }
}
$previousProxy = $previousServiceEnvironment |
    Where-Object { $_ -like 'GEMINI_BRIDGE_PROXY=*' } |
    Select-Object -First 1
$suggestedProxy = if ($persistedProxyKnown) {
    $persistedProxy
}
elseif (-not [string]::IsNullOrWhiteSpace($previousProxy)) {
    ($previousProxy -split '=', 2)[1]
}
else {
    Get-WindowsProxyUrl
}

if ($configureGemini -and
    -not $NonInteractive -and
    -not $proxyUrlSpecified -and
    -not $DirectConnection) {
    $prompt = if ([string]::IsNullOrWhiteSpace($suggestedProxy)) {
        '代理地址（输入 direct 使用直连，或输入代理 URL）'
    }
    else {
        "代理地址（回车使用 $suggestedProxy，输入 direct 使用直连）"
    }
    $proxyAnswer = (Read-Host $prompt).Trim()
    if ($proxyAnswer -eq 'direct') {
        $DirectConnection = $true
    }
    elseif ([string]::IsNullOrWhiteSpace($proxyAnswer)) {
        $ProxyUrl = $suggestedProxy
    }
    else {
        $ProxyUrl = $proxyAnswer
    }
}
elseif (-not $proxyUrlSpecified -and -not $DirectConnection) {
    $ProxyUrl = $suggestedProxy
}

if ($DirectConnection) {
    $ProxyUrl = $null
}
if (-not [string]::IsNullOrWhiteSpace($ProxyUrl)) {
    $proxyUri = $null
    if (-not [Uri]::TryCreate($ProxyUrl, [UriKind]::Absolute, [ref]$proxyUri) -or
        $proxyUri.Scheme -notin @('http', 'https')) {
        throw "代理地址无效或协议不受支持：$ProxyUrl"
    }
}

$existingService = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($null -ne $existingService -and $existingService.Status -ne 'Stopped') {
    Stop-Service -Name $serviceName -Force
    $existingService.WaitForStatus(
        [System.ServiceProcess.ServiceControllerStatus]::Stopped,
        [TimeSpan]::FromSeconds(30)
    )
}

$listeners = @(
    Get-NetTCPConnection `
        -LocalAddress '127.0.0.1' `
        -LocalPort $Port `
        -State Listen `
        -ErrorAction SilentlyContinue
)
if ($listeners.Count -gt 0) {
    throw "端口 127.0.0.1:$Port 已被其他程序占用，请先关闭该程序。"
}

New-Item -ItemType Directory -Path $installDir -Force | Out-Null
New-Item -ItemType Directory -Path $programDataDir -Force | Out-Null
New-Item -ItemType Directory -Path $scriptsDir -Force | Out-Null
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
New-Item -ItemType Directory -Path $imageDir -Force | Out-Null
New-Item -ItemType Directory -Path $claudeDir -Force | Out-Null

Copy-FileIfNeeded -Source $packageExe -Destination $serviceExe
Copy-FileIfNeeded -Source $packageGui -Destination $guiExe
Copy-FileIfNeeded `
    -Source (Join-Path $packageDir 'scripts\start-bridge.ps1') `
    -Destination (Join-Path $scriptsDir 'start-bridge.ps1')
Copy-FileIfNeeded `
    -Source (Join-Path $packageDir 'scripts\stop-bridge.ps1') `
    -Destination (Join-Path $scriptsDir 'stop-bridge.ps1')

$currentIdentity = [Security.Principal.WindowsIdentity]::GetCurrent()
if ($configureGemini) {
    $escapedApiKey = $apiKey.Replace('\', '\\').Replace('"', '\"')
    [System.IO.File]::WriteAllText(
        $keyFile,
        "experimental_bearer_token = `"$escapedApiKey`"`r`n",
        $utf8NoBom
    )
    $apiKey = $null

    $keyAcl = [Security.AccessControl.FileSecurity]::new()
    $keyAcl.SetAccessRuleProtection($true, $false)
    $keyPrincipals = @(
        @{
            Sid = [Security.Principal.SecurityIdentifier]::new('S-1-5-18')
            Rights = [Security.AccessControl.FileSystemRights]::FullControl
        }
        @{
            Sid = [Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')
            Rights = [Security.AccessControl.FileSystemRights]::FullControl
        }
        @{
            Sid = $currentIdentity.User
            Rights = [Security.AccessControl.FileSystemRights]::FullControl
        }
    )
    foreach ($principalEntry in $keyPrincipals) {
        $rule = [Security.AccessControl.FileSystemAccessRule]::new(
            $principalEntry.Sid,
            $principalEntry.Rights,
            [Security.AccessControl.AccessControlType]::Allow
        )
        $keyAcl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $keyFile -AclObject $keyAcl
}

$template = [System.IO.File]::ReadAllText($bridgeSettingsTemplate) | ConvertFrom-Json
$timestamp = Get-Date -Format 'yyyyMMdd-HHmmss'
if ($configureGemini) {
    $geminiTemplate = [ordered]@{
        name = 'Google Gemini'
        model = 'gemini-3.6-flash'
        base_url = "http://127.0.0.1:$Port"
        protocol = 'gemini'
        identity = 'Google Gemini (gemini-3.6-flash)'
    }
    if (Test-Path -LiteralPath $geminiProfile -PathType Leaf) {
        Copy-Item `
            -LiteralPath $geminiProfile `
            -Destination "$geminiProfile.backup-$timestamp" `
            -Force
    }
    [System.IO.File]::WriteAllText(
        $geminiProfile,
        ($geminiTemplate | ConvertTo-Json -Depth 8),
        $utf8NoBom
    )
}

if (Test-Path -LiteralPath $claudeSettings -PathType Leaf) {
    Copy-Item `
        -LiteralPath $claudeSettings `
        -Destination "$claudeSettings.backup-$timestamp" `
        -Force
    $settings = [System.IO.File]::ReadAllText($claudeSettings) | ConvertFrom-Json
}
else {
    $settings = [pscustomobject]@{}
}
if ($null -eq $settings.PSObject.Properties['env']) {
    $settings | Add-Member -MemberType NoteProperty -Name env -Value ([pscustomobject]@{})
}
elseif ($null -eq $settings.env) {
    $settings.env = [pscustomobject]@{}
}
foreach ($property in $template.env.PSObject.Properties) {
    $existingProperty = $settings.env.PSObject.Properties[$property.Name]
    if ($null -eq $existingProperty) {
        $settings.env | Add-Member `
            -MemberType NoteProperty `
            -Name $property.Name `
            -Value $property.Value
    }
    else {
        $existingProperty.Value = $property.Value
    }
}
[System.IO.File]::WriteAllText(
    $claudeSettings,
    ($settings | ConvertTo-Json -Depth 100),
    $utf8NoBom
)

try {
    if (Test-Path -LiteralPath $claudeUserConfig -PathType Leaf) {
        Copy-Item `
            -LiteralPath $claudeUserConfig `
            -Destination "$claudeUserConfig.backup-$timestamp" `
            -Force
        $claudeUser = [System.IO.File]::ReadAllText($claudeUserConfig) |
            ConvertFrom-Json
    }
    else {
        $claudeUser = [pscustomobject]@{}
    }
    if ($null -eq $claudeUser.PSObject.Properties['mcpServers']) {
        $claudeUser | Add-Member `
            -MemberType NoteProperty `
            -Name mcpServers `
            -Value ([pscustomobject]@{})
    }
    elseif ($null -eq $claudeUser.mcpServers) {
        $claudeUser.mcpServers = [pscustomobject]@{}
    }
    $imageMcp = [pscustomobject]@{
        type = 'http'
        url = "http://127.0.0.1:$Port/mcp"
    }
    $existingImageMcp = $claudeUser.mcpServers.PSObject.Properties['gemini-image']
    if ($null -eq $existingImageMcp) {
        $claudeUser.mcpServers | Add-Member `
            -MemberType NoteProperty `
            -Name 'gemini-image' `
            -Value $imageMcp
    }
    else {
        $existingImageMcp.Value = $imageMcp
    }
    [System.IO.File]::WriteAllText(
        $claudeUserConfig,
        ($claudeUser | ConvertTo-Json -Depth 100),
        $utf8NoBom
    )
}
catch {
    Write-Warning "无法自动注册 Gemini 生图工具：$($_.Exception.Message)"
}

$binaryPath = "`"$serviceExe`" --windows-service"
$scBinaryPath = '\"' + $serviceExe + '\" --windows-service'
if ($null -eq $existingService) {
    New-Service `
        -Name $serviceName `
        -BinaryPathName $binaryPath `
        -DisplayName $displayName `
        -Description 'Always-on local protocol bridge for Claude Code model providers.' `
        -StartupType Automatic | Out-Null
}
else {
    & sc.exe config $serviceName binPath= $scBinaryPath | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "更新服务配置失败，sc.exe 返回 $LASTEXITCODE。"
    }
}

& sc.exe config $serviceName obj= $serviceAccount | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "设置隔离服务账户失败，sc.exe 返回 $LASTEXITCODE。"
}
& sc.exe sidtype $serviceName unrestricted | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "启用服务 SID 失败，sc.exe 返回 $LASTEXITCODE。"
}

& sc.exe config $serviceName start= delayed-auto | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "设置自动启动失败，sc.exe 返回 $LASTEXITCODE。"
}
& sc.exe description $serviceName 'Always-on local protocol bridge for Claude Code model providers.' | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "设置服务说明失败，sc.exe 返回 $LASTEXITCODE。"
}
& sc.exe failure $serviceName reset= 86400 actions= restart/5000/restart/15000/restart/60000 | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "设置故障恢复失败，sc.exe 返回 $LASTEXITCODE。"
}
& sc.exe failureflag $serviceName 1 | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "启用故障恢复失败，sc.exe 返回 $LASTEXITCODE。"
}

$serviceSddlOutput = & sc.exe sdshow $serviceName
if ($LASTEXITCODE -ne 0) {
    throw "读取服务权限失败，sc.exe 返回 $LASTEXITCODE。"
}
$serviceSddl = $serviceSddlOutput |
    Where-Object { $_ -match '^D:' } |
    Select-Object -First 1
if ([string]::IsNullOrWhiteSpace($serviceSddl)) {
    throw 'Windows 返回了空的服务安全描述符。'
}
$currentUserSid = $currentIdentity.User.Value
if (-not $serviceSddl.Contains(";;;$currentUserSid)")) {
    $userControlAce = "(A;;LCRPWPLO;;;$currentUserSid)"
    $saclIndex = $serviceSddl.IndexOf('S:')
    if ($saclIndex -ge 0) {
        $serviceSddl = $serviceSddl.Insert($saclIndex, $userControlAce)
    }
    else {
        $serviceSddl += $userControlAce
    }
    & sc.exe sdset $serviceName $serviceSddl | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "授予 GUI 启停权限失败，sc.exe 返回 $LASTEXITCODE。"
    }
}

$serviceEnvironment = @(
    "GEMINI_BRIDGE_LISTEN=127.0.0.1:$Port"
    "GEMINI_BRIDGE_STATE_FILE=$stateFile"
    "GEMINI_BRIDGE_LOG_DIR=$logDir"
    "GEMINI_BRIDGE_IMAGE_DIR=$imageDir"
    "CLAUDE_SETTINGS_DIR=$claudeDir"
    "CLAUDE_BRIDGE_PROVIDERS_DIR=$providersDir"
    'RUST_LOG=claude_bridge=info,tower_http=info'
)
$keyProfileEntry = $null
if (Test-Path -LiteralPath $keyFile -PathType Leaf) {
    $keyProfileEntry = "GEMINI_BRIDGE_API_KEY_PROFILE=$keyFile"
}
elseif (-not $configureGemini) {
    $keyProfileEntry = $previousServiceEnvironment |
        Where-Object { $_ -like 'GEMINI_BRIDGE_API_KEY_PROFILE=*' } |
        Select-Object -First 1
    if ([string]::IsNullOrWhiteSpace($keyProfileEntry)) {
        $userProfileDir = Split-Path -Parent $claudeDir
        $legacyKeyProfile = Join-Path `
            $userProfileDir `
            '.codex\gemini35flash-aistudio.config.toml'
        if (Test-Path -LiteralPath $legacyKeyProfile -PathType Leaf) {
            $keyProfileEntry = "GEMINI_BRIDGE_API_KEY_PROFILE=$legacyKeyProfile"
        }
    }
}
if (-not [string]::IsNullOrWhiteSpace($keyProfileEntry)) {
    $serviceEnvironment += $keyProfileEntry
}
if (-not [string]::IsNullOrWhiteSpace($ProxyUrl)) {
    $serviceEnvironment += "GEMINI_BRIDGE_PROXY=$ProxyUrl"
}
New-ItemProperty `
    -LiteralPath $serviceRegistry `
    -Name Environment `
    -PropertyType MultiString `
    -Value $serviceEnvironment `
    -Force | Out-Null

$bridgeState = if (Test-Path -LiteralPath $stateFile -PathType Leaf) {
    try {
        Get-Content -LiteralPath $stateFile -Raw | ConvertFrom-Json
    }
    catch {
        [PSCustomObject]@{}
    }
}
else {
    [PSCustomObject]@{}
}
if ($null -eq $bridgeState.PSObject.Properties['active_profile']) {
    $bridgeState | Add-Member `
        -NotePropertyName active_profile `
        -NotePropertyValue 'gemini.json'
}
if ($null -eq $bridgeState.PSObject.Properties['gemini_proxy']) {
    $bridgeState | Add-Member -NotePropertyName gemini_proxy -NotePropertyValue $ProxyUrl
}
else {
    $bridgeState.gemini_proxy = $ProxyUrl
}
[System.IO.File]::WriteAllText(
    $stateFile,
    ($bridgeState | ConvertTo-Json -Depth 8 -Compress),
    $utf8NoBom
)

Grant-ServicePathAccess `
    -Path $installDir `
    -Rights ([Security.AccessControl.FileSystemRights]::ReadAndExecute)
Grant-ServicePathAccess `
    -Path $programDataDir `
    -Rights ([Security.AccessControl.FileSystemRights]::Modify)
Grant-ServicePathAccess `
    -Path $imageDir `
    -Rights ([Security.AccessControl.FileSystemRights]::Modify)
Grant-ServicePathAccess `
    -Path $claudeDir `
    -Rights ([Security.AccessControl.FileSystemRights]::ReadAndExecute)
Grant-ServicePathAccess `
    -Path $providersDir `
    -Rights ([Security.AccessControl.FileSystemRights]::ReadAndExecute)
if (-not [string]::IsNullOrWhiteSpace($keyProfileEntry)) {
    $serviceKeyProfile = ($keyProfileEntry -split '=', 2)[1]
    Grant-ServicePathAccess `
        -Path $serviceKeyProfile `
        -Rights ([Security.AccessControl.FileSystemRights]::Read)
}

if (-not $SkipShortcuts) {
    $desktop = [Environment]::GetFolderPath('DesktopDirectory')
    if (-not [string]::IsNullOrWhiteSpace($desktop)) {
        $shortcutPath = Join-Path $desktop 'Claude Code 模型切换器.lnk'
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut($shortcutPath)
        $shortcut.TargetPath = $guiExe
        $shortcut.WorkingDirectory = $installDir
        $shortcut.Description = 'Claude Code Multi-Model Bridge'
        $shortcut.Save()
    }
}

Start-Service -Name $serviceName
$service = Get-Service -Name $serviceName
$service.WaitForStatus(
    [System.ServiceProcess.ServiceControllerStatus]::Running,
    [TimeSpan]::FromSeconds(30)
)

$healthUrl = "http://127.0.0.1:$Port/health"
$deadline = [DateTime]::UtcNow.AddSeconds(30)
$healthy = $false
while ([DateTime]::UtcNow -lt $deadline) {
    try {
        $health = Invoke-RestMethod -Uri $healthUrl -TimeoutSec 3
        $healthy = $health.status -eq 'ok'
    }
    catch {
        $healthy = $false
    }
    if ($healthy) {
        break
    }
    Start-Sleep -Milliseconds 250
}
if (-not $healthy) {
    throw "服务已经启动，但健康检查失败：$healthUrl"
}

$resolvedLegacyServiceExe = [System.IO.Path]::GetFullPath($legacyServiceExe)
$resolvedServiceExe = [System.IO.Path]::GetFullPath($serviceExe)
if (
    -not [string]::Equals(
        $resolvedLegacyServiceExe,
        $resolvedServiceExe,
        [StringComparison]::OrdinalIgnoreCase
    ) -and
    (Test-Path -LiteralPath $resolvedLegacyServiceExe -PathType Leaf)
) {
    Remove-Item -LiteralPath $resolvedLegacyServiceExe -Force
}

Write-Host ''
Write-Host '安装成功：'
Write-Host "  服务：$serviceName（延迟自动启动）"
Write-Host "  地址：http://127.0.0.1:$Port"
Write-Host "  程序：$installDir"
Write-Host "  日志：$logDir"
Write-Host "  生图目录：$imageDir"
Write-Host "  Claude 配置：$claudeSettings"
Write-Host "  Provider 配置：$providersDir"
Write-Host "  Gemini：$(if ($configureGemini) { '已配置' } else { '未配置（可稍后添加）' })"
Write-Host ''
Write-Host '请重新启动正在运行的 Claude Code 会话，使环境配置和 Gemini 生图工具生效。'
