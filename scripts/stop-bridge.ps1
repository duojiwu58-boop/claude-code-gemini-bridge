param(
    [int]$Port = 18787
)

$ErrorActionPreference = 'Stop'

$serviceName = 'ClaudeCodeBridge'
$projectDir = Split-Path -Parent $PSScriptRoot
$pidPath = Join-Path $projectDir 'target\bridge.pid'
$localAuthToken = $env:GEMINI_BRIDGE_LOCAL_TOKEN
if ([string]::IsNullOrWhiteSpace($localAuthToken)) {
    $localTokenFile = Join-Path $env:ProgramData 'ClaudeCodeBridge\local-auth-token'
    if (Test-Path -LiteralPath $localTokenFile -PathType Leaf) {
        $localAuthToken = [System.IO.File]::ReadAllText($localTokenFile).Trim()
    }
}
$shutdownHeaders = @{}
if (-not [string]::IsNullOrWhiteSpace($localAuthToken)) {
    $shutdownHeaders.Authorization = "Bearer $localAuthToken"
}

$service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($null -ne $service) {
    if ($service.Status -eq 'Stopped') {
        Write-Output 'bridge_status=not_running'
        exit 0
    }

    $shutdownRequested = $false
    try {
        $shutdownResponse = Invoke-RestMethod `
            -Uri "http://127.0.0.1:$Port/admin/shutdown" `
            -Method Post `
            -Headers $shutdownHeaders `
            -TimeoutSec 5
        $shutdownRequested = $shutdownResponse.status -eq 'shutting_down'
    }
    catch {
        $shutdownRequested = $false
    }

    if (-not $shutdownRequested) {
        Stop-Service -Name $serviceName -Force
    }
    $service.WaitForStatus(
        [System.ServiceProcess.ServiceControllerStatus]::Stopped,
        [TimeSpan]::FromSeconds(30)
    )
    Write-Output 'bridge_shutdown=graceful'
    Write-Output 'bridge_status=stopped'
    exit 0
}

if (-not (Test-Path -LiteralPath $pidPath -PathType Leaf)) {
    Write-Output 'bridge_status=not_running'
    exit 0
}

$processIdText = [System.IO.File]::ReadAllText($pidPath).Trim()
$processId = 0
if (-not [int]::TryParse($processIdText, [ref]$processId)) {
    throw "Invalid PID file: $pidPath"
}

$process = Get-Process -Id $processId -ErrorAction SilentlyContinue
if ($null -ne $process) {
    $expectedExecutables = @(
        (Join-Path $projectDir 'target\x86_64-pc-windows-msvc\release\claude-bridge.exe')
        (Join-Path $projectDir 'target\x86_64-pc-windows-msvc\release\codex-gemini-bridge.exe')
    )
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

    if (-not [string]::IsNullOrWhiteSpace($actualPath)) {
        $resolvedActual = [System.IO.Path]::GetFullPath($actualPath)
        $matchesBridgeExecutable = $expectedExecutables |
            ForEach-Object {
                [string]::Equals(
                    $resolvedActual,
                    [System.IO.Path]::GetFullPath($_),
                    [System.StringComparison]::OrdinalIgnoreCase
                )
            } |
            Where-Object { $_ } |
            Select-Object -First 1
        if (-not $matchesBridgeExecutable) {
            throw "PID $processId does not belong to the bridge executable."
        }
    }
    elseif ($process.ProcessName -notin @('claude-bridge', 'codex-gemini-bridge')) {
        throw "PID $processId does not belong to the bridge process."
    }

    $shutdownRequested = $false
    try {
        $shutdownResponse = Invoke-RestMethod `
            -Uri "http://127.0.0.1:$Port/admin/shutdown" `
            -Method Post `
            -Headers $shutdownHeaders `
            -TimeoutSec 5
        $shutdownRequested = $shutdownResponse.status -eq 'shutting_down'
    }
    catch {
        $shutdownRequested = $false
    }

    if ($shutdownRequested) {
        $deadline = [DateTime]::UtcNow.AddSeconds(10)
        while (
            $null -ne (Get-Process -Id $processId -ErrorAction SilentlyContinue) -and
            [DateTime]::UtcNow -lt $deadline
        ) {
            Start-Sleep -Milliseconds 100
        }
    }

    $remainingProcess = Get-Process -Id $processId -ErrorAction SilentlyContinue
    if ($null -ne $remainingProcess) {
        Stop-Process -Id $processId
        Wait-Process -Id $processId -ErrorAction SilentlyContinue
        Write-Output 'bridge_shutdown=forced'
    }
    else {
        Write-Output 'bridge_shutdown=graceful'
    }
}

Remove-Item -LiteralPath $pidPath
Write-Output 'bridge_status=stopped'
