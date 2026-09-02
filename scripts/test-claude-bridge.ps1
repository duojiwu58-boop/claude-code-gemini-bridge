param(
    [int]$Port = 18787,
    [string]$ProfileFile = 'settings - gemini3.6 bridge.json',
    [string]$LocalTokenFile
)

$ErrorActionPreference = 'Stop'
$projectDir = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'local-auth.ps1')
$localAuthToken = Get-BridgeLocalAuthToken `
    -ProjectDir $projectDir `
    -TokenFile $LocalTokenFile
$adminHeaders = New-BridgeAuthorizationHeaders -Token $localAuthToken
$baseUrl = "http://127.0.0.1:$Port"
$status = Invoke-RestMethod `
    -Uri "$baseUrl/admin/status" `
    -Headers $adminHeaders `
    -TimeoutSec 5
$originalProfile = $status.active_profile.file
$switchBody = @{
    file = $ProfileFile
} | ConvertTo-Json
Invoke-RestMethod `
    -Uri "$baseUrl/admin/active-profile" `
    -Method Post `
    -Headers $adminHeaders `
    -ContentType 'application/json' `
    -Body $switchBody `
    -TimeoutSec 5 | Out-Null

try {
$headers = @{
    Authorization = "Bearer $localAuthToken"
    'Content-Type' = 'application/json'
    'anthropic-version' = '2023-06-01'
}
$uri = "http://127.0.0.1:$Port/v1/messages"
$tool = @{
    name = 'get_current_directory'
    description = 'Return the current working directory.'
    input_schema = @{
        type = 'object'
        properties = @{}
    }
}

$firstBody = @{
    model = 'gemini-3.8-flash'
    max_tokens = 512
    stream = $false
    thinking = @{
        type = 'adaptive'
    }
    messages = @(
        @{
            role = 'user'
            content = 'Call get_current_directory now.'
        }
    )
    tools = @($tool)
    tool_choice = @{
        type = 'tool'
        name = 'get_current_directory'
    }
} | ConvertTo-Json -Depth 12

$first = Invoke-RestMethod `
    -Uri $uri `
    -Method Post `
    -Headers $headers `
    -Body $firstBody `
    -TimeoutSec 90

$toolUse = $first.content | Where-Object { $_.type -eq 'tool_use' } | Select-Object -First 1
if ($null -eq $toolUse) {
    throw 'The first response did not contain an Anthropic tool_use block.'
}

$secondBody = @{
    model = 'gemini-3.8-flash'
    max_tokens = 512
    stream = $true
    thinking = @{
        type = 'adaptive'
    }
    messages = @(
        @{
            role = 'user'
            content = 'Call get_current_directory now.'
        },
        @{
            role = 'assistant'
            content = @($toolUse)
        },
        @{
            role = 'user'
            content = @(
                @{
                    type = 'text'
                    text = 'Use the tool result below.'
                },
                @{
                    type = 'tool_result'
                    tool_use_id = $toolUse.id
                    content = 'D:\rust_vfpar'
                },
                @{
                    type = 'text'
                    text = 'Reply with the returned directory.'
                }
            )
        }
    )
    tools = @($tool)
} | ConvertTo-Json -Depth 12

$second = Invoke-WebRequest `
    -Uri $uri `
    -Method Post `
    -Headers $headers `
    -Body $secondBody `
    -UseBasicParsing `
    -TimeoutSec 90

$hasMessageStop = $second.Content.Contains('event: message_stop')
$hasTextDelta = $second.Content.Contains('"type":"text_delta"')

Write-Output "first_stop_reason=$($first.stop_reason)"
Write-Output "tool_name=$($toolUse.name)"
Write-Output "stream_http_status=$($second.StatusCode)"
Write-Output "has_text_delta=$hasTextDelta"
Write-Output "has_message_stop=$hasMessageStop"

if (
    $first.stop_reason -ne 'tool_use' -or
    $toolUse.name -ne 'get_current_directory' -or
    -not $hasTextDelta -or
    -not $hasMessageStop
) {
    throw 'Claude Messages bridge verification failed.'
}
}
finally {
    $restoreBody = @{
        file = $originalProfile
    } | ConvertTo-Json
    Invoke-RestMethod `
        -Uri "$baseUrl/admin/active-profile" `
        -Method Post `
        -Headers $adminHeaders `
        -ContentType 'application/json' `
        -Body $restoreBody `
        -TimeoutSec 5 | Out-Null
    Write-Output "restored_profile=$originalProfile"
}
