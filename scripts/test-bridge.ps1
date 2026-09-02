param(
    [int]$Port = 18787,
    [string]$LocalTokenFile
)

$ErrorActionPreference = 'Stop'
$projectDir = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'local-auth.ps1')
$localAuthToken = Get-BridgeLocalAuthToken `
    -ProjectDir $projectDir `
    -TokenFile $LocalTokenFile
$headers = New-BridgeAuthorizationHeaders -Token $localAuthToken
$headers['Content-Type'] = 'application/json'

$body = @{
    model = 'gemini-3.8-flash'
    input = 'Reply with exactly OK.'
    stream = $true
    reasoning = @{
        effort = 'high'
    }
} | ConvertTo-Json -Depth 8

$response = Invoke-WebRequest `
    -Uri "http://127.0.0.1:$Port/v1/responses" `
    -Method Post `
    -Headers $headers `
    -Body $body `
    -UseBasicParsing `
    -TimeoutSec 60

if ($response.StatusCode -ne 200) {
    throw "Bridge returned HTTP $($response.StatusCode)"
}

$hasCompleted = $response.Content.Contains('response.completed')
$outputText = ''
foreach ($line in ($response.Content -split "`n")) {
    if (-not $line.StartsWith('data:')) {
        continue
    }
    $jsonText = $line.Substring(5).Trim()
    if ($jsonText.Length -eq 0) {
        continue
    }
    $eventData = $jsonText | ConvertFrom-Json
    if ($eventData.type -eq 'response.output_text.done') {
        $outputText = $eventData.text
    }
}

Write-Output "http_status=$($response.StatusCode)"
Write-Output "has_response_completed=$hasCompleted"
Write-Output "output_text=$outputText"

if (-not $hasCompleted -or [string]::IsNullOrWhiteSpace($outputText)) {
    throw 'Bridge response did not contain the expected Responses API events.'
}
