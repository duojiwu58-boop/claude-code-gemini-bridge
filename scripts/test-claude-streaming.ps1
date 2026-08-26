param(
    [int]$Port = 18787,
    [string]$LocalTokenFile
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Net.Http
$projectDir = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'local-auth.ps1')
$localAuthToken = Get-BridgeLocalAuthToken `
    -ProjectDir $projectDir `
    -TokenFile $LocalTokenFile

$body = @{
    model = 'gemini-3.7-flash'
    max_tokens = 2048
    stream = $true
    messages = @(
        @{
            role = 'user'
            content = 'Write exactly 40 numbered lines. Each line must contain one different short sentence.'
        }
    )
} | ConvertTo-Json -Depth 8

$handler = New-Object System.Net.Http.HttpClientHandler
$handler.UseProxy = $false
$client = New-Object System.Net.Http.HttpClient($handler)
$request = New-Object System.Net.Http.HttpRequestMessage(
    [System.Net.Http.HttpMethod]::Post,
    "http://127.0.0.1:$Port/v1/messages"
)
$request.Headers.TryAddWithoutValidation('Authorization', "Bearer $localAuthToken") | Out-Null
$request.Headers.TryAddWithoutValidation('anthropic-version', '2023-06-01') | Out-Null
$request.Content = New-Object System.Net.Http.StringContent(
    $body,
    [System.Text.Encoding]::UTF8,
    'application/json'
)

$timer = [System.Diagnostics.Stopwatch]::StartNew()
$response = $client.SendAsync(
    $request,
    [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead
).GetAwaiter().GetResult()

if (-not $response.IsSuccessStatusCode) {
    $errorBody = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
    throw "Bridge returned HTTP $([int]$response.StatusCode): $errorBody"
}

$responseStream = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
$reader = New-Object System.IO.StreamReader(
    $responseStream,
    [System.Text.Encoding]::UTF8
)
$firstTextDeltaMs = $null
$textDeltaCount = 0
$hasMessageStop = $false

while (($line = $reader.ReadLine()) -ne $null) {
    if ($line.Contains('"type":"text_delta"')) {
        $textDeltaCount += 1
        if ($null -eq $firstTextDeltaMs) {
            $firstTextDeltaMs = $timer.ElapsedMilliseconds
        }
    }
    if ($line.Contains('"type":"message_stop"')) {
        $hasMessageStop = $true
        break
    }
}

$totalMs = $timer.ElapsedMilliseconds
$reader.Dispose()
$response.Dispose()
$request.Dispose()
$client.Dispose()
$handler.Dispose()

Write-Output "first_text_delta_ms=$firstTextDeltaMs"
Write-Output "total_stream_ms=$totalMs"
Write-Output "text_delta_count=$textDeltaCount"
Write-Output "has_message_stop=$hasMessageStop"

if (
    $null -eq $firstTextDeltaMs -or
    $textDeltaCount -le 1 -or
    -not $hasMessageStop -or
    $firstTextDeltaMs -ge $totalMs
) {
    throw 'The response did not demonstrate incremental upstream streaming.'
}
