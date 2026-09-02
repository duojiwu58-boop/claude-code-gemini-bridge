param(
    [int]$Port = 18787
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
        [int]$TimeoutSec = 5
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
$localAuthToken = $env:GEMINI_BRIDGE_LOCAL_TOKEN
if (Test-StringNullOrWhitespace ($localAuthToken)) {
    $localTokenFile = Join-Path $env:ProgramData 'ClaudeCodeBridge\local-auth-token'
    if ([System.IO.File]::Exists($localTokenFile)) {
        $localAuthToken = [System.IO.File]::ReadAllText($localTokenFile).Trim()
    }
}
$shutdownHeaders = @{}
if (-not (Test-StringNullOrWhitespace ($localAuthToken))) {
    $shutdownHeaders['Authorization'] = "Bearer $localAuthToken"
}
$service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($null -eq $service -or $service.Status -eq 'Stopped') {
    Write-Output 'bridge_status=not_running'
    exit 0
}

$shutdownRequested = $false
try {
    $response = Invoke-RestMethodCompat `
        -Uri "http://127.0.0.1:$Port/admin/shutdown" `
        -Method Post `
        -Headers $shutdownHeaders `
        -TimeoutSec 5
    if ($response -is [System.Collections.IDictionary]) {
        $shutdownRequested = $response['status'] -eq 'shutting_down'
    }
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
