param(
    [int]$Port = 18787
)

$ErrorActionPreference = 'Stop'
$serviceName = 'ClaudeCodeBridge'
$service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($null -eq $service) {
    throw 'ClaudeCodeBridge Windows 服务尚未安装。'
}
if ($Port -ne 18787) {
    throw '发布版 Windows 服务固定使用端口 18787。'
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
while ([DateTime]::UtcNow -lt $deadline) {
    try {
        $health = Invoke-RestMethod -Uri $healthUrl -TimeoutSec 3
        if ($health.status -eq 'ok') {
            Write-Output 'service_status=Running'
            Write-Output "health_url=$healthUrl"
            exit 0
        }
    }
    catch {
    }
    Start-Sleep -Milliseconds 250
}
throw "服务已经启动，但健康检查失败：$healthUrl"
