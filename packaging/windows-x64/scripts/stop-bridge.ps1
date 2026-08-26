param(
    [int]$Port = 18787
)

$ErrorActionPreference = 'Stop'
$serviceName = 'ClaudeCodeBridge'
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
if ($null -eq $service -or $service.Status -eq 'Stopped') {
    Write-Output 'bridge_status=not_running'
    exit 0
}

$shutdownRequested = $false
try {
    $response = Invoke-RestMethod `
        -Uri "http://127.0.0.1:$Port/admin/shutdown" `
        -Method Post `
        -Headers $shutdownHeaders `
        -TimeoutSec 5
    $shutdownRequested = $response.status -eq 'shutting_down'
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
