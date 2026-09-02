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
    [switch]$SkipShortcuts,
    [string]$ElevationUserProfile,
    [string]$ElevationUserPictures,
    [string]$ElevationUserDesktop,
    [string]$ElevationUserSid
)

$ErrorActionPreference = 'Stop'
$proxyUrlSpecified = $PSBoundParameters.ContainsKey('ProxyUrl')

# ---------------------------------------------------------------------------
# PowerShell 2.0 compatibility helpers.
#
# Windows 7 SP1 ships Windows PowerShell 2.0 only. The helpers below provide
# JSON conversion, HTTP requests, string checks, and a TCP listener probe in
# a PS 2.0-compatible way (System.Web.Extensions JavaScriptSerializer, raw
# HttpWebRequest, and netstat.exe). Keep this script PS 2.0-safe: no ::new(),
# no [pscustomobject], no ConvertTo/From-Json, no -in/-notin, no hashtable
# dot access ($h.key), no -LiteralPath/-PathType/-File.
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

function Invoke-RestMethodCompat {
    param(
        [Parameter(Mandatory)]
        [string]$Uri,
        [string]$Method = 'GET',
        [hashtable]$Headers = @{},
        [int]$TimeoutSec = 3
    )
    $request = [System.Net.HttpWebRequest]::Create($Uri)
    $request.Method = $Method
    $request.Timeout = $TimeoutSec * 1000
    $request.ReadWriteTimeout = $TimeoutSec * 1000
    if ($null -ne $Headers) {
        foreach ($key in $Headers.Keys) {
            if ($key -eq 'Content-Type') {
                $request.ContentType = [string]$Headers[$key]
                continue
            }
            if ($key -eq 'User-Agent') {
                $request.UserAgent = [string]$Headers[$key]
                continue
            }
            $request.Headers.Add([string]$key, [string]$Headers[$key])
        }
    }
    $response = $request.GetResponse()
    try {
        $reader = New-Object -TypeName System.IO.StreamReader -ArgumentList @(
            $response.GetResponseStream()
        )
        $body = $reader.ReadToEnd()
    }
    finally {
        $reader.Dispose()
        $response.Close()
    }
    return ConvertFrom-JsonCompat $body
}

function Test-TcpListenerActive {
    param([int]$Port)
    $netstatLines = & netstat.exe -ano -p tcp
    foreach ($line in $netstatLines) {
        if ($line -match ('^\s*TCP\s+127\.0\.0\.1:' + $Port + '\s+\S+\s+LISTENING\s+\d+\s*$')) {
            return $true
        }
    }
    return $false
}

if ($DirectConnection -and $proxyUrlSpecified) {
    throw '-DirectConnection 不能与 -ProxyUrl 同时使用。'
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
        throw '使用命令行参数安装时，请先打开“管理员 PowerShell”。'
    }
    $unelevatedIdentity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $unelevatedProfile = [System.IO.Path]::GetFullPath($env:USERPROFILE)
    $unelevatedPictures = [Environment]::GetFolderPath(
        [Environment+SpecialFolder]::MyPictures
    )
    $unelevatedDesktop = [Environment]::GetFolderPath('DesktopDirectory')
    $elevationArguments = (
        (
            '-NoProfile -ExecutionPolicy Bypass -File "{0}" ' +
            '-ElevationUserProfile "{1}" -ElevationUserPictures "{2}" ' +
            '-ElevationUserDesktop "{3}" -ElevationUserSid "{4}"'
        ) -f
        $MyInvocation.MyCommand.Path,
        $unelevatedProfile,
        $unelevatedPictures,
        $unelevatedDesktop,
        $unelevatedIdentity.User.Value
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
    Copy-Item -Path $sourcePath -Destination $destinationPath -Force
}

function Get-WindowsProxyUrl {
    $internetSettings = Get-ItemProperty `
        -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' `
        -ErrorAction SilentlyContinue
    if ($null -eq $internetSettings -or
        $internetSettings.ProxyEnable -ne 1 -or
        (Test-StringNullOrWhitespace ([string]$internetSettings.ProxyServer))) {
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
            $candidates['default'] = $trimmed
        }
    }
    $detected = if ($candidates.ContainsKey('https')) {
        $candidates['https']
    }
    elseif ($candidates.ContainsKey('http')) {
        $candidates['http']
    }
    elseif ($candidates.ContainsKey('default')) {
        $candidates['default']
    }
    else {
        $null
    }
    if (Test-StringNullOrWhitespace ($detected)) {
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
$packageDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$packageExe = Join-Path $packageDir 'claude-bridge.exe'
$packageComputerHost = Join-Path $packageDir 'claude-computer-host.exe'
$packageGui = Join-Path $packageDir 'ClaudeBridgeManager.exe'
$bridgeSettingsTemplate = Join-Path $packageDir 'claude-settings.bridge.json'
$installDir = Join-Path $env:ProgramFiles 'ClaudeCodeBridge'
$programDataDir = Join-Path $env:ProgramData 'ClaudeCodeBridge'
$serviceExe = Join-Path $installDir 'claude-bridge.exe'
$computerHostExe = Join-Path $installDir 'claude-computer-host.exe'
$legacyServiceExe = Join-Path $installDir 'codex-gemini-bridge.exe'
$guiExe = Join-Path $installDir 'ClaudeBridgeManager.exe'
$scriptsDir = Join-Path $installDir 'scripts'
$logDir = Join-Path $programDataDir 'logs'
$keyFile = Join-Path $programDataDir 'gemini-api-key.toml'
$localTokenFile = Join-Path $programDataDir 'local-auth-token'
$installMetadataFile = Join-Path $programDataDir 'install-metadata.json'
$stateFile = Join-Path $programDataDir 'bridge-state.json'
$targetUserProfile = if (Test-StringNullOrWhitespace ($ElevationUserProfile)) {
    [System.IO.Path]::GetFullPath($env:USERPROFILE)
}
else {
    [System.IO.Path]::GetFullPath($ElevationUserProfile)
}
$claudeDir = if (Test-StringNullOrWhitespace ($ClaudeSettingsDir)) {
    Join-Path $targetUserProfile '.claude'
}
else {
    [System.IO.Path]::GetFullPath($ClaudeSettingsDir)
}
$claudeSettings = Join-Path $claudeDir 'settings.json'
$claudeUserConfig = Join-Path (Split-Path -Parent $claudeDir) '.claude.json'
$picturesDir = if (Test-StringNullOrWhitespace ($ElevationUserPictures)) {
    [Environment]::GetFolderPath([Environment+SpecialFolder]::MyPictures)
}
else {
    [System.IO.Path]::GetFullPath($ElevationUserPictures)
}
if (Test-StringNullOrWhitespace ($picturesDir)) {
    $picturesDir = Join-Path (Split-Path -Parent $claudeDir) 'Pictures'
}
$imageDir = Join-Path $picturesDir 'ClaudeCodeBridge'
$desktopDir = if (Test-StringNullOrWhitespace ($ElevationUserDesktop)) {
    [Environment]::GetFolderPath('DesktopDirectory')
}
else {
    [System.IO.Path]::GetFullPath($ElevationUserDesktop)
}
$shortcutPath = if (Test-StringNullOrWhitespace ($desktopDir)) {
    $null
}
else {
    Join-Path $desktopDir 'Claude Code 模型切换器.lnk'
}
$providersDir = if (Test-StringNullOrWhitespace ($ProviderConfigDir)) {
    Join-Path $claudeDir 'bridge-providers'
}
else {
    [System.IO.Path]::GetFullPath($ProviderConfigDir)
}
$geminiProfile = Join-Path $providersDir 'gemini.json'
$serviceRegistry = "HKLM:\SYSTEM\CurrentControlSet\Services\$serviceName"
$utf8NoBom = New-Object -TypeName System.Text.UTF8Encoding -ArgumentList @($false)
New-Item -ItemType Directory -Path $providersDir -Force | Out-Null

function Grant-ServicePathAccess {
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [Security.AccessControl.FileSystemRights]$Rights
    )

    if (-not (Test-Path -Path $Path)) {
        return
    }
    $item = Get-Item -Path $Path
    $inheritance = if ($item.PSIsContainer) {
        [Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
        [Security.AccessControl.InheritanceFlags]::ObjectInherit
    }
    else {
        [Security.AccessControl.InheritanceFlags]::None
    }
    $acl = Get-Acl -Path $Path
    $rule = New-Object -TypeName Security.AccessControl.FileSystemAccessRule -ArgumentList @(
        $serviceAccount,
        $Rights,
        $inheritance,
        [Security.AccessControl.PropagationFlags]::None,
        [Security.AccessControl.AccessControlType]::Allow
    )
    $acl.SetAccessRule($rule)
    Set-Acl -Path $Path -AclObject $acl
}

function Grant-ServiceDirectoryBrowseAccess {
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    if (-not [System.IO.Directory]::Exists($Path)) {
        return
    }
    $acl = Get-Acl -Path $Path
    $rule = New-Object -TypeName Security.AccessControl.FileSystemAccessRule -ArgumentList @(
        $serviceAccount,
        [Security.AccessControl.FileSystemRights]::ReadAndExecute,
        [Security.AccessControl.InheritanceFlags]::None,
        [Security.AccessControl.PropagationFlags]::None,
        [Security.AccessControl.AccessControlType]::Allow
    )
    $acl.SetAccessRule($rule)
    Set-Acl -Path $Path -AclObject $acl
}

function Remove-ServicePathAccess {
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    if (-not (Test-Path -Path $Path)) {
        return
    }
    $acl = Get-Acl -Path $Path
    $serviceIdentity = New-Object -TypeName Security.Principal.NTAccount -ArgumentList @($serviceAccount)
    $acl.PurgeAccessRules($serviceIdentity)
    Set-Acl -Path $Path -AclObject $acl
}

function New-LocalAuthToken {
    $bytes = New-Object byte[] 32
    $random = [Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $random.GetBytes($bytes)
    }
    finally {
        $random.Dispose()
    }
    return [Convert]::ToBase64String($bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_')
}

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

function Write-ProtectedUtf8TextAtomically {
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string]$Contents,
        [Parameter(Mandatory)]
        [Security.AccessControl.FileSecurity]$Acl
    )
    $temporaryPath = "$Path.tmp-$PID-$([Guid]::NewGuid().ToString('N'))"
    $stream = $null
    try {
        $stream = New-Object -TypeName System.IO.FileStream -ArgumentList @(
            $temporaryPath,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::None
        )
        Set-Acl -Path $temporaryPath -AclObject $Acl
        $bytes = $utf8NoBom.GetBytes($Contents)
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush($true)
        $stream.Dispose()
        $stream = $null
        Move-Item -Path $temporaryPath -Destination $Path -Force
    }
    finally {
        if ($null -ne $stream) {
            $stream.Dispose()
        }
        if ([System.IO.File]::Exists($temporaryPath)) {
            [System.IO.File]::Delete($temporaryPath)
        }
    }
}

function Restore-ManagedServiceConfiguration {
    param(
        $Snapshot,
        [bool]$CreatedByThisRun
    )

    Stop-Service -Name $serviceName -Force -ErrorAction SilentlyContinue
    if ($CreatedByThisRun) {
        & sc.exe delete $serviceName | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "删除安装失败后创建的服务失败，sc.exe 返回 $LASTEXITCODE。"
        }
        return
    }
    if ($null -eq $Snapshot) {
        return
    }

    & sc.exe config $serviceName binPath= $Snapshot.ImagePath | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "恢复服务程序路径失败，sc.exe 返回 $LASTEXITCODE。"
    }
    if (-not (Test-StringNullOrWhitespace ($Snapshot.ObjectName))) {
        & sc.exe config $serviceName obj= $Snapshot.ObjectName | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "恢复服务账户失败，sc.exe 返回 $LASTEXITCODE。"
        }
    }
    $startMode = switch ($Snapshot.Start) {
        2 {
            if ($Snapshot.DelayedAutoStart) { 'delayed-auto' } else { 'auto' }
        }
        3 { 'demand' }
        4 { 'disabled' }
        default { 'demand' }
    }
    & sc.exe config $serviceName start= $startMode | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "恢复服务启动类型失败，sc.exe 返回 $LASTEXITCODE。"
    }
    if (-not (Test-StringNullOrWhitespace ($Snapshot.DisplayName))) {
        & sc.exe config $serviceName DisplayName= $Snapshot.DisplayName | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "恢复服务显示名称失败，sc.exe 返回 $LASTEXITCODE。"
        }
    }
    & sc.exe description $serviceName ([string]$Snapshot.Description) | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "恢复服务说明失败，sc.exe 返回 $LASTEXITCODE。"
    }
    if (-not (Test-StringNullOrWhitespace ($Snapshot.Sddl))) {
        & sc.exe sdset $serviceName $Snapshot.Sddl | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "恢复服务权限失败，sc.exe 返回 $LASTEXITCODE。"
        }
    }

    foreach ($property in @(
        @{ Name = 'Environment'; Type = 'MultiString' }
        @{ Name = 'FailureActions'; Type = 'Binary' }
        @{ Name = 'FailureActionsOnNonCrashFailures'; Type = 'DWord' }
    )) {
        $snapshotProperty = $Snapshot.Registry.PSObject.Properties[$property['Name']]
        if ($null -eq $snapshotProperty) {
            Remove-ItemProperty `
                -Path $serviceRegistry `
                -Name $property['Name'] `
                -ErrorAction SilentlyContinue
        }
        else {
            New-ItemProperty `
                -Path $serviceRegistry `
                -Name $property['Name'] `
                -PropertyType $property['Type'] `
                -Value $snapshotProperty.Value `
                -Force | Out-Null
        }
    }
    if ($Snapshot.WasRunning) {
        Start-Service -Name $serviceName
    }
}

if ($Port -ne 18787) {
    throw '当前 GUI 发布版固定使用端口 18787。'
}

$requiredFiles = @(
    $packageExe
    $packageComputerHost
    $packageGui
    $bridgeSettingsTemplate
    (Join-Path $packageDir 'scripts\start-bridge.ps1')
    (Join-Path $packageDir 'scripts\stop-bridge.ps1')
)
foreach ($requiredFile in $requiredFiles) {
    if (-not [System.IO.File]::Exists($requiredFile)) {
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
    if (-not (Test-StringNullOrWhitespace ($ApiKeyFile))) {
        if (-not [System.IO.File]::Exists($ApiKeyFile)) {
            throw "找不到安装器提供的 API Key 临时文件：$ApiKeyFile"
        }
        try {
            $apiKey = [System.IO.File]::ReadAllText($ApiKeyFile).Trim()
        }
        finally {
            Remove-Item -Path $ApiKeyFile -Force -ErrorAction SilentlyContinue
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
    if (Test-StringNullOrWhitespace ($apiKey)) {
        throw 'API Key 不能为空。'
    }
}

$previousServiceEnvironment = @()
if (Test-Path -Path $serviceRegistry) {
    $previousServiceEnvironment = @(
        (
            Get-ItemProperty `
                -Path $serviceRegistry `
                -ErrorAction SilentlyContinue
        ).Environment
    )
}

$persistedProxyKnown = $false
$persistedProxy = $null
if ([System.IO.File]::Exists($stateFile)) {
    try {
        $previousState = ConvertFrom-JsonCompat ([System.IO.File]::ReadAllText($stateFile))
        if ($previousState -is [System.Collections.IDictionary] -and
            $previousState.ContainsKey('gemini_proxy')) {
            $persistedProxyKnown = $true
            $persistedProxy = [string]$previousState['gemini_proxy']
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
elseif (-not (Test-StringNullOrWhitespace ($previousProxy))) {
    ($previousProxy -split '=', 2)[1]
}
else {
    Get-WindowsProxyUrl
}

if ($configureGemini -and
    -not $NonInteractive -and
    -not $proxyUrlSpecified -and
    -not $DirectConnection) {
    $prompt = if (Test-StringNullOrWhitespace ($suggestedProxy)) {
        '代理地址（输入 direct 使用直连，或输入代理 URL）'
    }
    else {
        "代理地址（回车使用 $suggestedProxy，输入 direct 使用直连）"
    }
    $proxyAnswer = (Read-Host $prompt).Trim()
    if ($proxyAnswer -eq 'direct') {
        $DirectConnection = $true
    }
    elseif (Test-StringNullOrWhitespace ($proxyAnswer)) {
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
if (-not (Test-StringNullOrWhitespace ($ProxyUrl))) {
    $proxyUri = $null
    if (-not [Uri]::TryCreate($ProxyUrl, [UriKind]::Absolute, [ref]$proxyUri) -or
        @('http', 'https') -notcontains $proxyUri.Scheme) {
        throw "代理地址无效或协议不受支持：$ProxyUrl"
    }
}

$existingService = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
$serviceRollback = $null
$serviceCreatedByThisRun = $false
if ($null -ne $existingService) {
    $existingServiceRegistry = Get-ItemProperty -Path $serviceRegistry
    $existingServiceSddlOutput = & sc.exe sdshow $serviceName
    if ($LASTEXITCODE -ne 0) {
        throw "读取原服务权限失败，sc.exe 返回 $LASTEXITCODE。"
    }
    $existingServiceSddl = $existingServiceSddlOutput |
        Where-Object { $_ -match '^D:' } |
        Select-Object -First 1
    $serviceRollback = New-Object PSObject -Property @{
        ImagePath = [string]$existingServiceRegistry.ImagePath
        ObjectName = [string]$existingServiceRegistry.ObjectName
        Start = [int]$existingServiceRegistry.Start
        DelayedAutoStart = [bool]$existingServiceRegistry.DelayedAutostart
        DisplayName = [string]$existingService.DisplayName
        Description = [string]$existingServiceRegistry.Description
        Sddl = [string]$existingServiceSddl
        Registry = $existingServiceRegistry
        WasRunning = $existingService.Status -ne 'Stopped'
    }
}
try {
if ($null -ne $existingService -and $existingService.Status -ne 'Stopped') {
    Stop-Service -Name $serviceName -Force
    $existingService.WaitForStatus(
        [System.ServiceProcess.ServiceControllerStatus]::Stopped,
        [TimeSpan]::FromSeconds(30)
    )
}

if (Test-TcpListenerActive -Port $Port) {
    throw "端口 127.0.0.1:$Port 已被其他程序占用，请先关闭该程序。"
}

New-Item -ItemType Directory -Path $installDir -Force | Out-Null
New-Item -ItemType Directory -Path $programDataDir -Force | Out-Null
New-Item -ItemType Directory -Path $scriptsDir -Force | Out-Null
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
New-Item -ItemType Directory -Path $imageDir -Force | Out-Null
New-Item -ItemType Directory -Path $claudeDir -Force | Out-Null

$currentIdentity = [Security.Principal.WindowsIdentity]::GetCurrent()
$installUserSid = if (Test-StringNullOrWhitespace ($ElevationUserSid)) {
    $currentIdentity.User
}
else {
    New-Object -TypeName Security.Principal.SecurityIdentifier -ArgumentList @($ElevationUserSid)
}
$installSecretAcl = New-Object -TypeName Security.AccessControl.FileSecurity
$installSecretAcl.SetAccessRuleProtection($true, $false)
foreach ($principalEntry in @(
    @{
        Sid = (New-Object -TypeName Security.Principal.SecurityIdentifier -ArgumentList @('S-1-5-18'))
        Rights = [Security.AccessControl.FileSystemRights]::FullControl
    }
    @{
        Sid = (New-Object -TypeName Security.Principal.SecurityIdentifier -ArgumentList @('S-1-5-32-544'))
        Rights = [Security.AccessControl.FileSystemRights]::FullControl
    }
    @{
        Sid = $installUserSid
        Rights = [Security.AccessControl.FileSystemRights]::FullControl
    }
)) {
    $installSecretAcl.AddAccessRule((New-Object -TypeName Security.AccessControl.FileSystemAccessRule -ArgumentList @(
        $principalEntry['Sid'],
        $principalEntry['Rights'],
        [Security.AccessControl.AccessControlType]::Allow
    )))
}

$localAuthToken = if ([System.IO.File]::Exists($localTokenFile)) {
    [System.IO.File]::ReadAllText($localTokenFile).Trim()
}
else {
    ''
}
if ($localAuthToken.Length -lt 32) {
    $localAuthToken = New-LocalAuthToken
    Write-ProtectedUtf8TextAtomically `
        -Path $localTokenFile `
        -Contents "$localAuthToken`r`n" `
        -Acl $installSecretAcl
}
else {
    Set-Acl -Path $localTokenFile -AclObject $installSecretAcl
}

Copy-FileIfNeeded -Source $packageExe -Destination $serviceExe
Copy-FileIfNeeded -Source $packageComputerHost -Destination $computerHostExe
Copy-FileIfNeeded -Source $packageGui -Destination $guiExe
Copy-FileIfNeeded `
    -Source (Join-Path $packageDir 'scripts\start-bridge.ps1') `
    -Destination (Join-Path $scriptsDir 'start-bridge.ps1')
Copy-FileIfNeeded `
    -Source (Join-Path $packageDir 'scripts\stop-bridge.ps1') `
    -Destination (Join-Path $scriptsDir 'stop-bridge.ps1')

if ($configureGemini) {
    $escapedApiKey = $apiKey.Replace('\', '\\').Replace('"', '\"')
    Write-ProtectedUtf8TextAtomically `
        -Path $keyFile `
        -Contents "experimental_bearer_token = `"$escapedApiKey`"`r`n" `
        -Acl $installSecretAcl
    $apiKey = $null
}

$template = ConvertFrom-JsonCompat ([System.IO.File]::ReadAllText($bridgeSettingsTemplate))
$template['env']['ANTHROPIC_AUTH_TOKEN'] = $localAuthToken
$timestamp = Get-Date -Format 'yyyyMMdd-HHmmss'
if ($configureGemini) {
    $geminiCapabilities = New-Object System.Collections.Specialized.OrderedDictionary
    $geminiCapabilities.Add('max_output_tokens', 65536)
    $geminiCapabilities.Add('gemini_store', $true)
    $geminiCapabilities.Add('gemini_service_tier', 'auto')
    $geminiCapabilities.Add('gemini_tool_choice_override', 'validated')
    $geminiTemplate = New-Object System.Collections.Specialized.OrderedDictionary
    $geminiTemplate.Add('name', 'Google Gemini')
    $geminiTemplate.Add('model', 'gemini-3.8-flash')
    $geminiTemplate.Add('context_window', 1048576)
    $geminiTemplate.Add('base_url', 'https://generativelanguage.googleapis.com/v1beta')
    $geminiTemplate.Add('protocol', 'gemini-interactions')
    $geminiTemplate.Add('bridge_managed_credentials', $true)
    $geminiTemplate.Add('identity', 'Google Gemini (gemini-3.8-flash)')
    $geminiTemplate.Add('capabilities', $geminiCapabilities)
    if ([System.IO.File]::Exists($geminiProfile)) {
        Copy-Item `
            -Path $geminiProfile `
            -Destination "$geminiProfile.backup-$timestamp" `
            -Force
    }
    Write-Utf8TextAtomically `
        -Path $geminiProfile `
        -Contents (ConvertTo-JsonCompat ($geminiTemplate))
}

if ([System.IO.File]::Exists($claudeSettings)) {
    Copy-Item `
        -Path $claudeSettings `
        -Destination "$claudeSettings.backup-$timestamp" `
        -Force
    $settings = ConvertFrom-JsonCompat ([System.IO.File]::ReadAllText($claudeSettings))
}
else {
    $settings = @{}
}
if (-not $settings.ContainsKey('env')) {
    $settings['env'] = @{}
}
elseif ($null -eq $settings['env']) {
    $settings['env'] = @{}
}
$previousEnv = $null
if ([System.IO.File]::Exists($installMetadataFile)) {
    try {
        $existingInstallMetadata = ConvertFrom-JsonCompat (
            [System.IO.File]::ReadAllText($installMetadataFile)
        )
        if ($existingInstallMetadata -is [System.Collections.IDictionary] -and
            $existingInstallMetadata.ContainsKey('previous_env')) {
            $previousEnv = @($existingInstallMetadata['previous_env'])
        }
    }
    catch {
        Write-Warning "无法读取旧安装元数据，将从当前 Claude 设置建立新的恢复点：$($_.Exception.Message)"
    }
}
if ($null -eq $previousEnv) {
    $previousEnv = @(
        foreach ($propertyName in $template['env'].Keys) {
            $snapshotValue = $null
            if ($settings['env'].ContainsKey($propertyName)) {
                $snapshotValue = $settings['env'][$propertyName]
            }
            (New-Object PSObject -Property @{
                name = $propertyName
                existed = $settings['env'].ContainsKey($propertyName)
                value = $snapshotValue
            })
        }
    )
}
foreach ($propertyName in $template['env'].Keys) {
    $settings['env'][$propertyName] = $template['env'][$propertyName]
}
Write-Utf8TextAtomically `
    -Path $claudeSettings `
    -Contents (ConvertTo-JsonCompat ($settings))
$installedEnv = @(
    foreach ($propertyName in $template['env'].Keys) {
        (New-Object PSObject -Property @{
            name = $propertyName
            value = $template['env'][$propertyName]
        })
    }
)
$installMetadata = New-Object PSObject -Property @{
    version = 1
    claude_settings = $claudeSettings
    previous_env = $previousEnv
    installed_env = $installedEnv
}
Write-ProtectedUtf8TextAtomically `
    -Path $installMetadataFile `
    -Contents (ConvertTo-JsonCompat ($installMetadata)) `
    -Acl $installSecretAcl

try {
    if ([System.IO.File]::Exists($claudeUserConfig)) {
        Copy-Item `
            -Path $claudeUserConfig `
            -Destination "$claudeUserConfig.backup-$timestamp" `
            -Force
        $claudeUser = ConvertFrom-JsonCompat ([System.IO.File]::ReadAllText($claudeUserConfig))
    }
    else {
        $claudeUser = @{}
    }
    if (-not $claudeUser.ContainsKey('mcpServers')) {
        $claudeUser['mcpServers'] = @{}
    }
    elseif ($null -eq $claudeUser['mcpServers']) {
        $claudeUser['mcpServers'] = @{}
    }
    $imageMcp = New-Object PSObject -Property @{
        type = 'http'
        url = "http://127.0.0.1:$Port/mcp"
        headers = (New-Object PSObject -Property @{
            Authorization = "Bearer $localAuthToken"
        })
    }
    $claudeUser['mcpServers']['gemini-image'] = $imageMcp
    $computerMcp = New-Object PSObject -Property @{
        type = 'stdio'
        command = $computerHostExe
        args = @('--stdio-mcp')
    }
    $claudeUser['mcpServers']['gemini-computer'] = $computerMcp
    Write-Utf8TextAtomically `
        -Path $claudeUserConfig `
        -Contents (ConvertTo-JsonCompat ($claudeUser))
}
catch {
    Write-Warning "无法自动注册 Gemini 生图或 Computer Use MCP 工具：$($_.Exception.Message)"
}

$binaryPath = "`"$serviceExe`" --windows-service"
if ($null -eq $existingService) {
    New-Service `
        -Name $serviceName `
        -BinaryPathName $binaryPath `
        -DisplayName $displayName `
        -Description 'Always-on local protocol bridge for Claude Code model providers.' `
        -StartupType Automatic | Out-Null
    $serviceCreatedByThisRun = $true
}
else {
    & sc.exe config $serviceName binPath= $binaryPath | Out-Null
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
if (Test-StringNullOrWhitespace ($serviceSddl)) {
    throw 'Windows 返回了空的服务安全描述符。'
}
$currentUserSid = $installUserSid.Value
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
    "GEMINI_BRIDGE_LOCAL_TOKEN_FILE=$localTokenFile"
    "GEMINI_BRIDGE_STATE_FILE=$stateFile"
    "GEMINI_BRIDGE_LOG_DIR=$logDir"
    "GEMINI_BRIDGE_IMAGE_DIR=$imageDir"
    "CLAUDE_SETTINGS_DIR=$claudeDir"
    "CLAUDE_BRIDGE_PROVIDERS_DIR=$providersDir"
    'RUST_LOG=claude_bridge=info,tower_http=info'
)
$keyProfileEntry = $null
if ([System.IO.File]::Exists($keyFile)) {
    $keyProfileEntry = "GEMINI_BRIDGE_API_KEY_PROFILE=$keyFile"
}
elseif (-not $configureGemini) {
    $keyProfileEntry = $previousServiceEnvironment |
        Where-Object { $_ -like 'GEMINI_BRIDGE_API_KEY_PROFILE=*' } |
        Select-Object -First 1
    if (Test-StringNullOrWhitespace ($keyProfileEntry)) {
        $userProfileDir = Split-Path -Parent $claudeDir
        $legacyKeyProfile = Join-Path `
            $userProfileDir `
            '.codex\gemini35flash-aistudio.config.toml'
        if ([System.IO.File]::Exists($legacyKeyProfile)) {
            $keyProfileEntry = "GEMINI_BRIDGE_API_KEY_PROFILE=$legacyKeyProfile"
        }
    }
}
if (-not (Test-StringNullOrWhitespace ($keyProfileEntry))) {
    $serviceEnvironment += $keyProfileEntry
}
if (-not (Test-StringNullOrWhitespace ($ProxyUrl))) {
    $serviceEnvironment += "GEMINI_BRIDGE_PROXY=$ProxyUrl"
}
New-ItemProperty `
    -Path $serviceRegistry `
    -Name Environment `
    -PropertyType MultiString `
    -Value $serviceEnvironment `
    -Force | Out-Null

$bridgeState = @{}
if ([System.IO.File]::Exists($stateFile)) {
    try {
        $parsedState = ConvertFrom-JsonCompat ([System.IO.File]::ReadAllText($stateFile))
        if ($parsedState -is [System.Collections.IDictionary]) {
            $bridgeState = $parsedState
        }
    }
    catch {
    }
}
if (-not $bridgeState.ContainsKey('active_profile')) {
    $bridgeState['active_profile'] = 'gemini.json'
}
$bridgeState['gemini_proxy'] = $ProxyUrl
Write-Utf8TextAtomically `
    -Path $stateFile `
    -Contents (ConvertTo-JsonCompat ($bridgeState))

$localTokenAcl = New-Object -TypeName Security.AccessControl.FileSecurity
$localTokenAcl.SetAccessRuleProtection($true, $false)
$localTokenPrincipals = @(
    @{
        Identity = (New-Object -TypeName Security.Principal.SecurityIdentifier -ArgumentList @('S-1-5-18'))
        Rights = [Security.AccessControl.FileSystemRights]::FullControl
    }
    @{
        Identity = $installUserSid
        Rights = [Security.AccessControl.FileSystemRights]::FullControl
    }
    @{
        Identity = $serviceAccount
        Rights = [Security.AccessControl.FileSystemRights]::Read
    }
)
foreach ($principalEntry in $localTokenPrincipals) {
    $rule = New-Object -TypeName Security.AccessControl.FileSystemAccessRule -ArgumentList @(
        $principalEntry['Identity'],
        $principalEntry['Rights'],
        [Security.AccessControl.AccessControlType]::Allow
    )
    $localTokenAcl.AddAccessRule($rule)
}
Set-Acl -Path $localTokenFile -AclObject $localTokenAcl
Set-Acl -Path $installMetadataFile -AclObject $localTokenAcl

Grant-ServicePathAccess `
    -Path $installDir `
    -Rights ([Security.AccessControl.FileSystemRights]::ReadAndExecute)
Grant-ServicePathAccess `
    -Path $programDataDir `
    -Rights ([Security.AccessControl.FileSystemRights]::Modify)
Grant-ServicePathAccess `
    -Path $localTokenFile `
    -Rights ([Security.AccessControl.FileSystemRights]::Read)
Grant-ServicePathAccess `
    -Path $imageDir `
    -Rights ([Security.AccessControl.FileSystemRights]::Modify)
Remove-ServicePathAccess -Path $claudeDir
Grant-ServiceDirectoryBrowseAccess -Path $claudeDir
Grant-ServicePathAccess `
    -Path $claudeSettings `
    -Rights ([Security.AccessControl.FileSystemRights]::Read)
Grant-ServicePathAccess `
    -Path $providersDir `
    -Rights ([Security.AccessControl.FileSystemRights]::ReadAndExecute)
Get-ChildItem -Path $claudeDir -Filter 'settings - *.json' |
    Where-Object { -not $_.PSIsContainer } |
    ForEach-Object {
        Grant-ServicePathAccess `
            -Path $_.FullName `
            -Rights ([Security.AccessControl.FileSystemRights]::Read)
    }
if (-not (Test-StringNullOrWhitespace ($keyProfileEntry))) {
    $serviceKeyProfile = ($keyProfileEntry -split '=', 2)[1]
    Grant-ServicePathAccess `
        -Path $serviceKeyProfile `
        -Rights ([Security.AccessControl.FileSystemRights]::Read)
}

if (-not $SkipShortcuts) {
    if (-not (Test-StringNullOrWhitespace ($shortcutPath))) {
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut($shortcutPath)
        $shortcut.TargetPath = $guiExe
        $shortcut.WorkingDirectory = $installDir
        $shortcut.Description = 'Claude Code Multi-Model Bridge'
        $shortcut.Save()
    }
}

$service = Get-Service -Name $serviceName
$serviceStartDeadline = [DateTime]::UtcNow.AddSeconds(60)
$lastServiceStartError = $null
while ([DateTime]::UtcNow -lt $serviceStartDeadline) {
    $service.Refresh()
    if ($service.Status -eq [System.ServiceProcess.ServiceControllerStatus]::Running) {
        break
    }
    if ($service.Status -eq [System.ServiceProcess.ServiceControllerStatus]::Stopped) {
        try {
            Start-Service -Name $serviceName -ErrorAction Stop
            $lastServiceStartError = $null
        }
        catch {
            # An upgrade can overlap a recovery action that was queued before the
            # installer stopped the old service. Refresh and retry instead of
            # reporting a failed install while SCM is already restarting it.
            $lastServiceStartError = $_
        }
    }
    Start-Sleep -Milliseconds 500
}
$service.Refresh()
if ($service.Status -ne [System.ServiceProcess.ServiceControllerStatus]::Running) {
    $serviceStartDetail = if ($null -eq $lastServiceStartError) {
        "当前状态为 $($service.Status)"
    }
    else {
        $lastServiceStartError.Exception.Message
    }
    throw "服务无法在 60 秒内启动：$serviceStartDetail"
}

$healthUrl = "http://127.0.0.1:$Port/health"
$deadline = [DateTime]::UtcNow.AddSeconds(30)
$healthy = $false
while ([DateTime]::UtcNow -lt $deadline) {
    try {
        $health = Invoke-RestMethodCompat -Uri $healthUrl -TimeoutSec 3
        if ($health -is [System.Collections.IDictionary]) {
            $healthy = $health['status'] -eq 'ok'
        }
        else {
            $healthy = $false
        }
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
    Stop-Service -Name $serviceName -Force -ErrorAction SilentlyContinue
    throw "服务已经启动，但健康检查失败：$healthUrl"
}
}
catch {
    $installFailure = $_
    try {
        Restore-ManagedServiceConfiguration `
            -Snapshot $serviceRollback `
            -CreatedByThisRun $serviceCreatedByThisRun
    }
    catch {
        Write-Warning "安装失败后的服务回滚也失败：$($_.Exception.Message)"
    }
    throw $installFailure
}

$resolvedLegacyServiceExe = [System.IO.Path]::GetFullPath($legacyServiceExe)
$resolvedServiceExe = [System.IO.Path]::GetFullPath($serviceExe)
if (
    -not [string]::Equals(
        $resolvedLegacyServiceExe,
        $resolvedServiceExe,
        [StringComparison]::OrdinalIgnoreCase
    ) -and
    [System.IO.File]::Exists($resolvedLegacyServiceExe)
) {
    Remove-Item -Path $resolvedLegacyServiceExe -Force
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
Write-Host "  Computer Host：$computerHostExe（由 Claude Code 通过 stdio MCP 自动启动）"
Write-Host ''
Write-Host '请重新启动正在运行的 Claude Code 会话，使环境配置、生图和 Computer Use 工具生效。'
