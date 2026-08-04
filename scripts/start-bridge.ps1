param(
    [string]$ProxyUrl = 'http://127.0.0.1:8080',
    [int]$Port = 18787,
    [string]$ApiKeyProfile = (
        Join-Path $env:USERPROFILE '.codex\gemini35flash-aistudio.config.toml'
    )
)

$ErrorActionPreference = 'Stop'

$serviceName = 'ClaudeCodeBridge'
$projectDir = Split-Path -Parent $PSScriptRoot
$exePath = Join-Path $projectDir 'target\x86_64-pc-windows-msvc\release\claude-bridge.exe'
$stdoutPath = Join-Path $projectDir 'target\bridge.stdout.log'
$stderrPath = Join-Path $projectDir 'target\bridge.stderr.log'
$pidPath = Join-Path $projectDir 'target\bridge.pid'

$service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($null -ne $service) {
    if ($Port -ne 18787) {
        throw 'The installed Windows service is configured for port 18787.'
    }
    if ($service.Status -ne 'Running') {
        Start-Service -Name $serviceName
        $service.WaitForStatus(
            [System.ServiceProcess.ServiceControllerStatus]::Running,
            [TimeSpan]::FromSeconds(30)
        )
    }
    $healthUrl = "http://127.0.0.1:$Port/health"
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        try {
            $health = Invoke-RestMethod -Uri $healthUrl -TimeoutSec 3
            if ($health.status -eq 'ok') {
                Write-Output "service_name=$serviceName"
                Write-Output 'service_status=Running'
                Write-Output "health_url=$healthUrl"
                exit 0
            }
        }
        catch {
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Windows service started but health check failed: $healthUrl"
}

if (-not (Test-Path -LiteralPath $exePath -PathType Leaf)) {
    throw "Bridge executable does not exist: $exePath"
}

$existing = Get-NetTCPConnection `
    -LocalAddress '127.0.0.1' `
    -LocalPort $Port `
    -State Listen `
    -ErrorAction SilentlyContinue

if ($null -ne $existing) {
    throw "Port 127.0.0.1:$Port is already in use."
}

$profileText = [System.IO.File]::ReadAllText($ApiKeyProfile)
$keyMatch = [regex]::Match(
    $profileText,
    '(?m)^experimental_bearer_token\s*=\s*"([^"]+)"\s*$'
)
if (-not $keyMatch.Success) {
    throw "API key not found in $ApiKeyProfile"
}

$env:GEMINI_BRIDGE_LISTEN = "127.0.0.1:$Port"
$env:GEMINI_BRIDGE_PROXY = $ProxyUrl
$env:GEMINI_API_KEY = $keyMatch.Groups[1].Value

$process = Start-Process `
    -FilePath $exePath `
    -WorkingDirectory $projectDir `
    -WindowStyle Hidden `
    -RedirectStandardOutput $stdoutPath `
    -RedirectStandardError $stderrPath `
    -PassThru

[System.IO.File]::WriteAllText($pidPath, $process.Id.ToString())

Write-Output "bridge_pid=$($process.Id)"
Write-Output "health_url=http://127.0.0.1:$Port/health"
