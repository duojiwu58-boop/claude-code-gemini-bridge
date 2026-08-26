param(
    [string]$ProxyUrl = 'http://127.0.0.1:8080',
    [int]$Port = 18787,
    [string]$ApiKeyProfile = (
        Join-Path $env:USERPROFILE '.codex\gemini35flash-aistudio.config.toml'
    ),
    [string]$ClaudeSettingsDir = (
        Join-Path $env:USERPROFILE '.claude'
    ),
    [string]$ProviderConfigDir,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

$serviceName = 'ClaudeCodeBridge'
$serviceAccount = "NT SERVICE\$serviceName"
$displayName = 'Claude Code Multi-Model Bridge'
$projectDir = Split-Path -Parent $PSScriptRoot
$buildTargetDir = Join-Path $projectDir 'target\service-build'
$releaseExe = Join-Path $buildTargetDir 'x86_64-pc-windows-msvc\release\claude-bridge.exe'
$projectReleaseExe = Join-Path $projectDir 'target\x86_64-pc-windows-msvc\release\claude-bridge.exe'
$legacyExe = Join-Path $projectDir 'target\x86_64-pc-windows-msvc\release\codex-gemini-bridge.exe'
$serviceDir = Join-Path $projectDir 'service'
$serviceExe = Join-Path $serviceDir 'claude-bridge.exe'
$localTokenFile = Join-Path $serviceDir 'local-auth-token'
$legacyServiceExe = Join-Path $serviceDir 'codex-gemini-bridge.exe'
$logDir = Join-Path $serviceDir 'logs'
$picturesDir = [Environment]::GetFolderPath(
    [Environment+SpecialFolder]::MyPictures
)
if ([string]::IsNullOrWhiteSpace($picturesDir)) {
    $picturesDir = Join-Path (Split-Path -Parent $ClaudeSettingsDir) 'Pictures'
}
$imageDir = Join-Path $picturesDir 'ClaudeCodeBridge'
$stateFile = Join-Path $projectDir 'bridge-state.json'
$legacyPidFile = Join-Path $projectDir 'target\bridge.pid'
$registryPath = "HKLM:\SYSTEM\CurrentControlSet\Services\$serviceName"
. (Join-Path $PSScriptRoot 'local-auth.ps1')

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

function Grant-ServiceDirectoryBrowseAccess {
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        return
    }
    $acl = Get-Acl -LiteralPath $Path
    $rule = [Security.AccessControl.FileSystemAccessRule]::new(
        $serviceAccount,
        [Security.AccessControl.FileSystemRights]::ReadAndExecute,
        [Security.AccessControl.InheritanceFlags]::None,
        [Security.AccessControl.PropagationFlags]::None,
        [Security.AccessControl.AccessControlType]::Allow
    )
    $acl.SetAccessRule($rule)
    Set-Acl -LiteralPath $Path -AclObject $acl
}

function Remove-ServicePathAccess {
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }
    $acl = Get-Acl -LiteralPath $Path
    $serviceIdentity = [Security.Principal.NTAccount]::new($serviceAccount)
    $acl.PurgeAccessRules($serviceIdentity)
    Set-Acl -LiteralPath $Path -AclObject $acl
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Installing the Windows service requires an elevated PowerShell process.'
}

if (-not (Test-Path -LiteralPath $ApiKeyProfile -PathType Leaf)) {
    throw "API key profile does not exist: $ApiKeyProfile"
}
$profileText = [System.IO.File]::ReadAllText($ApiKeyProfile)
$keyMatch = [regex]::Match(
    $profileText,
    '(?m)^experimental_bearer_token\s*=\s*"([^"]+)"\s*$'
)
if (-not $keyMatch.Success) {
    throw "API key not found in $ApiKeyProfile"
}
if (-not (Test-Path -LiteralPath $ClaudeSettingsDir -PathType Container)) {
    throw "Claude settings directory does not exist: $ClaudeSettingsDir"
}
$resolvedProviderConfigDir = if ([string]::IsNullOrWhiteSpace($ProviderConfigDir)) {
    Join-Path $ClaudeSettingsDir 'bridge-providers'
}
else {
    [System.IO.Path]::GetFullPath($ProviderConfigDir)
}
New-Item -ItemType Directory -Path $resolvedProviderConfigDir -Force | Out-Null
New-Item -ItemType Directory -Path $imageDir -Force | Out-Null

if (-not $SkipBuild) {
    $env:CARGO_TARGET_DIR = $buildTargetDir
    & cargo build --locked --release --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) {
        throw "Cargo release build failed with exit code $LASTEXITCODE."
    }
}
if (-not (Test-Path -LiteralPath $releaseExe -PathType Leaf)) {
    throw "Bridge release executable does not exist: $releaseExe"
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
foreach ($listener in $listeners) {
    $processId = [int]$listener.OwningProcess
    $process = Get-Process -Id $processId -ErrorAction SilentlyContinue
    if ($null -eq $process) {
        continue
    }
    $actualPath = $null
    try {
        $actualPath = $process.Path
    }
    catch {
        $actualPath = $null
    }
    if ([string]::IsNullOrWhiteSpace($actualPath)) {
        $processInfo = Get-CimInstance `
            -ClassName Win32_Process `
            -Filter "ProcessId = $processId" `
            -ErrorAction SilentlyContinue
        $actualPath = $processInfo.ExecutablePath
    }

    $allowedPaths = @(
        $releaseExe
        $projectReleaseExe
        $legacyExe
        $serviceExe
        $legacyServiceExe
    ) | ForEach-Object {
        [System.IO.Path]::GetFullPath($_)
    }
    if (
        [string]::IsNullOrWhiteSpace($actualPath) -or
        -not ($allowedPaths -contains [System.IO.Path]::GetFullPath($actualPath))
    ) {
        throw "Port 127.0.0.1:$Port is owned by an unrelated process (PID $processId)."
    }

    try {
        $shutdownHeaders = @{}
        try {
            $existingLocalAuthToken = Get-BridgeLocalAuthToken -ProjectDir $projectDir
            $shutdownHeaders = New-BridgeAuthorizationHeaders -Token $existingLocalAuthToken
        }
        catch {
        }
        Invoke-RestMethod `
            -Uri "http://127.0.0.1:$Port/admin/shutdown" `
            -Method Post `
            -Headers $shutdownHeaders `
            -TimeoutSec 5 | Out-Null
    }
    catch {
        Stop-Process -Id $processId
    }

    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while (
        $null -ne (Get-Process -Id $processId -ErrorAction SilentlyContinue) -and
        [DateTime]::UtcNow -lt $deadline
    ) {
        Start-Sleep -Milliseconds 100
    }
    if ($null -ne (Get-Process -Id $processId -ErrorAction SilentlyContinue)) {
        Stop-Process -Id $processId
        Wait-Process -Id $processId -ErrorAction SilentlyContinue
    }
}
if (Test-Path -LiteralPath $legacyPidFile -PathType Leaf) {
    Remove-Item -LiteralPath $legacyPidFile -Force
}

New-Item -ItemType Directory -Path $serviceDir -Force | Out-Null
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$localAuthToken = Get-BridgeLocalAuthToken `
    -ProjectDir $projectDir `
    -TokenFile $localTokenFile `
    -CreateDevelopmentToken
Copy-Item -LiteralPath $releaseExe -Destination $serviceExe -Force

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
        throw "Failed to update service configuration (sc.exe exit $LASTEXITCODE)."
    }
}

& sc.exe config $serviceName obj= $serviceAccount | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Failed to configure the isolated service account (sc.exe exit $LASTEXITCODE)."
}
& sc.exe sidtype $serviceName unrestricted | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Failed to enable the service SID (sc.exe exit $LASTEXITCODE)."
}

& sc.exe config $serviceName start= delayed-auto | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Failed to enable delayed automatic startup (sc.exe exit $LASTEXITCODE)."
}
& sc.exe description $serviceName 'Always-on local protocol bridge for Claude Code model providers.' | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Failed to set service description (sc.exe exit $LASTEXITCODE)."
}
& sc.exe failure $serviceName reset= 86400 actions= restart/5000/restart/15000/restart/60000 | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Failed to set service recovery actions (sc.exe exit $LASTEXITCODE)."
}
& sc.exe failureflag $serviceName 1 | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Failed to enable recovery for non-crash failures (sc.exe exit $LASTEXITCODE)."
}

# The Delphi GUI runs without elevation. Grant only the installing user the
# right to query/start/stop this one service; configuration and deletion remain
# administrator-only.
$serviceSddlOutput = & sc.exe sdshow $serviceName
if ($LASTEXITCODE -ne 0) {
    throw "Failed to read service permissions (sc.exe exit $LASTEXITCODE)."
}
$serviceSddl = $serviceSddlOutput |
    Where-Object { $_ -match '^D:' } |
    Select-Object -First 1
if ([string]::IsNullOrWhiteSpace($serviceSddl)) {
    throw 'Windows returned an empty service security descriptor.'
}
$currentUserSid = $identity.User.Value
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
        throw "Failed to grant GUI service controls (sc.exe exit $LASTEXITCODE)."
    }
}

$serviceEnvironment = @(
    "GEMINI_BRIDGE_LISTEN=127.0.0.1:$Port"
    "GEMINI_BRIDGE_LOCAL_TOKEN_FILE=$localTokenFile"
    "GEMINI_BRIDGE_PROXY=$ProxyUrl"
    "GEMINI_BRIDGE_API_KEY_PROFILE=$ApiKeyProfile"
    "GEMINI_BRIDGE_STATE_FILE=$stateFile"
    "GEMINI_BRIDGE_LOG_DIR=$logDir"
    "GEMINI_BRIDGE_IMAGE_DIR=$imageDir"
    "CLAUDE_SETTINGS_DIR=$ClaudeSettingsDir"
    "CLAUDE_BRIDGE_PROVIDERS_DIR=$resolvedProviderConfigDir"
    'RUST_LOG=claude_bridge=info,tower_http=info'
)
New-ItemProperty `
    -LiteralPath $registryPath `
    -Name Environment `
    -PropertyType MultiString `
    -Value $serviceEnvironment `
    -Force | Out-Null

if (-not (Test-Path -LiteralPath $stateFile -PathType Leaf)) {
    [System.IO.File]::WriteAllText(
        $stateFile,
        '{}',
        [System.Text.UTF8Encoding]::new($false)
    )
}
Grant-ServicePathAccess `
    -Path $serviceDir `
    -Rights ([Security.AccessControl.FileSystemRights]::ReadAndExecute)
Grant-ServicePathAccess `
    -Path $logDir `
    -Rights ([Security.AccessControl.FileSystemRights]::Modify)
Grant-ServicePathAccess `
    -Path $imageDir `
    -Rights ([Security.AccessControl.FileSystemRights]::Modify)
Remove-ServicePathAccess -Path $ClaudeSettingsDir
Grant-ServiceDirectoryBrowseAccess -Path $ClaudeSettingsDir
Grant-ServicePathAccess `
    -Path (Join-Path $ClaudeSettingsDir 'settings.json') `
    -Rights ([Security.AccessControl.FileSystemRights]::Read)
Grant-ServicePathAccess `
    -Path $resolvedProviderConfigDir `
    -Rights ([Security.AccessControl.FileSystemRights]::ReadAndExecute)
Get-ChildItem -LiteralPath $ClaudeSettingsDir -File -Filter 'settings - *.json' |
    ForEach-Object {
        Grant-ServicePathAccess `
            -Path $_.FullName `
            -Rights ([Security.AccessControl.FileSystemRights]::Read)
    }
Grant-ServicePathAccess `
    -Path $ApiKeyProfile `
    -Rights ([Security.AccessControl.FileSystemRights]::Read)
Grant-ServicePathAccess `
    -Path $localTokenFile `
    -Rights ([Security.AccessControl.FileSystemRights]::Read)
Grant-ServicePathAccess `
    -Path $stateFile `
    -Rights ([Security.AccessControl.FileSystemRights]::Modify)

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
    throw "Service is running but its health endpoint did not become ready: $healthUrl"
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

Write-Output "service_name=$serviceName"
Write-Output 'startup_type=AutomaticDelayedStart'
Write-Output "binary_path=$serviceExe"
Write-Output "health_url=$healthUrl"
Write-Output 'service_status=Running'
