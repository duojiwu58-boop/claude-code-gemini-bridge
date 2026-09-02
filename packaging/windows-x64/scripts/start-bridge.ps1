param(
    [int]$Port = 18787
)

$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# PowerShell 2.0 (Windows 7 SP1) compatible helpers.
# ---------------------------------------------------------------------------

function ConvertFrom-JsonCompat {
    param([Parameter(Mandatory)][string]$InputObject)
    Add-Type -AssemblyName System.Web.Extensions -ErrorAction SilentlyContinue
    $serializer = New-Object System.Web.Script.Serialization.JavaScriptSerializer
    $serializer.MaxJsonLength = [int]::MaxValue
    return $serializer.DeserializeObject($InputObject)
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
        $health = Invoke-RestMethodCompat -Uri $healthUrl -TimeoutSec 3
        if ($health -is [System.Collections.IDictionary] -and
            $health['status'] -eq 'ok') {
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
