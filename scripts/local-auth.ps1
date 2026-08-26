function Get-BridgeLocalAuthToken {
    param(
        [Parameter(Mandatory)]
        [string]$ProjectDir,
        [string]$TokenFile,
        [switch]$CreateDevelopmentToken
    )

    $managedTokenFileRequested = $CreateDevelopmentToken -and
        -not [string]::IsNullOrWhiteSpace($TokenFile)
    if (-not $managedTokenFileRequested -and
        -not [string]::IsNullOrWhiteSpace($env:GEMINI_BRIDGE_LOCAL_TOKEN)) {
        $token = $env:GEMINI_BRIDGE_LOCAL_TOKEN.Trim()
        if ($token.Length -lt 32) {
            throw 'GEMINI_BRIDGE_LOCAL_TOKEN must contain at least 32 characters.'
        }
        return $token
    }

    $candidateFiles = [Collections.Generic.List[string]]::new()
    $candidates = if ($managedTokenFileRequested) {
        @($TokenFile)
    }
    else {
        @(
            $TokenFile
            $env:GEMINI_BRIDGE_LOCAL_TOKEN_FILE
            (Join-Path $env:ProgramData 'ClaudeCodeBridge\local-auth-token')
            (Join-Path $ProjectDir 'target\local-auth-token')
        )
    }
    foreach ($candidate in $candidates) {
        if (-not [string]::IsNullOrWhiteSpace($candidate)) {
            $resolved = [System.IO.Path]::GetFullPath($candidate)
            if (-not $candidateFiles.Contains($resolved)) {
                $candidateFiles.Add($resolved)
            }
        }
    }

    foreach ($candidate in $candidateFiles) {
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            continue
        }
        $token = [System.IO.File]::ReadAllText($candidate).Trim()
        if ($token.Length -lt 32) {
            throw "Bridge local token file must contain at least 32 characters: $candidate"
        }
        return $token
    }

    if (-not $CreateDevelopmentToken) {
        throw (
            'Bridge local token was not found. Start the bridge with scripts\start-bridge.ps1 ' +
            'or set GEMINI_BRIDGE_LOCAL_TOKEN(_FILE).'
        )
    }

    $developmentTokenFile = if (-not [string]::IsNullOrWhiteSpace($TokenFile)) {
        [System.IO.Path]::GetFullPath($TokenFile)
    }
    else {
        [System.IO.Path]::GetFullPath((Join-Path $ProjectDir 'target\local-auth-token'))
    }
    [System.IO.Directory]::CreateDirectory(
        [System.IO.Path]::GetDirectoryName($developmentTokenFile)
    ) | Out-Null
    $bytes = [byte[]]::new(32)
    $random = [Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $random.GetBytes($bytes)
    }
    finally {
        $random.Dispose()
    }
    $token = [Convert]::ToBase64String($bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_')
    $acl = [Security.AccessControl.FileSecurity]::new()
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($identity in @(
        [Security.Principal.SecurityIdentifier]::new('S-1-5-18')
        [Security.Principal.WindowsIdentity]::GetCurrent().User
    )) {
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            $identity,
            [Security.AccessControl.FileSystemRights]::FullControl,
            [Security.AccessControl.AccessControlType]::Allow
        ))
    }
    $temporaryTokenFile = "$developmentTokenFile.tmp-$PID-$([Guid]::NewGuid().ToString('N'))"
    $stream = $null
    try {
        $stream = [System.IO.FileStream]::new(
            $temporaryTokenFile,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::None
        )
        Set-Acl -LiteralPath $temporaryTokenFile -AclObject $acl
        $encodedToken = [System.Text.UTF8Encoding]::new($false).GetBytes("$token`r`n")
        $stream.Write($encodedToken, 0, $encodedToken.Length)
        $stream.Flush($true)
        $stream.Dispose()
        $stream = $null
        [System.IO.File]::Move($temporaryTokenFile, $developmentTokenFile)
    }
    finally {
        if ($null -ne $stream) {
            $stream.Dispose()
        }
        if ([System.IO.File]::Exists($temporaryTokenFile)) {
            [System.IO.File]::Delete($temporaryTokenFile)
        }
    }
    return $token
}

function New-BridgeAuthorizationHeaders {
    param(
        [Parameter(Mandatory)]
        [string]$Token
    )
    return @{ Authorization = "Bearer $Token" }
}
