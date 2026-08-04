param(
    [string]$ProfileFile = 'settings - ds4.json',
    [int]$Port = 18787,
    [switch]$OmitAnthropicVersion
)

$ErrorActionPreference = 'Stop'
$baseUrl = "http://127.0.0.1:$Port"
$status = Invoke-RestMethod -Uri "$baseUrl/admin/status" -TimeoutSec 5
$originalProfile = $status.active_profile.file

try {
    $switchBody = @{
        file = $ProfileFile
    } | ConvertTo-Json
    Invoke-RestMethod `
        -Uri "$baseUrl/admin/active-profile" `
        -Method Post `
        -ContentType 'application/json' `
        -Body $switchBody `
        -TimeoutSec 5 | Out-Null

    $headers = @{
        Authorization = 'Bearer local-gemini-bridge'
    }
    if (-not $OmitAnthropicVersion) {
        $headers['anthropic-version'] = '2023-06-01'
    }
    $messageBody = @{
        model = 'bridge-selected-model'
        max_tokens = 128
        stream = $false
        messages = @(
            @{
                role = 'user'
                content = 'Reply with exactly PROFILE_ROUTE_OK.'
            }
        )
    } | ConvertTo-Json -Depth 8
    $response = Invoke-RestMethod `
        -Uri "$baseUrl/v1/messages" `
        -Method Post `
        -Headers $headers `
        -ContentType 'application/json' `
        -Body $messageBody `
        -TimeoutSec 90
    $text = (
        $response.content |
        Where-Object { $_.type -eq 'text' } |
        Select-Object -First 1
    ).text

    Write-Output "profile=$ProfileFile"
    Write-Output "response_model=$($response.model)"
    Write-Output "response_text=$text"
    if ([string]::IsNullOrWhiteSpace($text)) {
        throw 'Provider route returned no text content.'
    }
}
finally {
    $restoreBody = @{
        file = $originalProfile
    } | ConvertTo-Json
    Invoke-RestMethod `
        -Uri "$baseUrl/admin/active-profile" `
        -Method Post `
        -ContentType 'application/json' `
        -Body $restoreBody `
        -TimeoutSec 5 | Out-Null
    Write-Output "restored_profile=$originalProfile"
}
