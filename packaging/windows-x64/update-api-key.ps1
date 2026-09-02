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

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object -TypeName Security.Principal.WindowsPrincipal -ArgumentList @($identity)
    return $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator
    )
}

if (-not (Test-IsAdministrator)) {
    $elevationArguments = (
        '-NoProfile -ExecutionPolicy Bypass -File "{0}"' -f $MyInvocation.MyCommand.Path
    )
    $elevated = Start-Process `
        -FilePath 'powershell.exe' `
        -ArgumentList $elevationArguments `
        -Verb RunAs `
        -Wait `
        -PassThru
    exit $elevated.ExitCode
}

$serviceName = 'ClaudeCodeBridge'
if ($null -eq (Get-Service -Name $serviceName -ErrorAction SilentlyContinue)) {
    throw 'ClaudeCodeBridge 服务尚未安装。'
}

$installScript = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) 'install.ps1'
if (-not [System.IO.File]::Exists($installScript)) {
    throw "找不到 Gemini 配置程序：$installScript"
}

$secureApiKey = Read-Host '请输入新的 Google AI Studio Gemini API Key' -AsSecureString
$keyPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secureApiKey)
try {
    $apiKey = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($keyPointer)
}
finally {
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($keyPointer)
}
if (Test-StringNullOrWhitespace ($apiKey)) {
    throw 'API Key 不能为空。'
}

$temporaryKeyFile = Join-Path (
    [System.IO.Path]::GetTempPath()
) "claude-bridge-key-$([Guid]::NewGuid().ToString('N')).txt"
try {
    [System.IO.File]::WriteAllText(
        $temporaryKeyFile,
        $apiKey,
        (New-Object -TypeName System.Text.UTF8Encoding -ArgumentList @($false))
    )
    $apiKey = $null
    & $installScript `
        -GeminiMode Configure `
        -ApiKeyFile $temporaryKeyFile `
        -ClaudeSettingsDir (Join-Path $env:USERPROFILE '.claude')
}
finally {
    $apiKey = $null
    if ([System.IO.File]::Exists($temporaryKeyFile)) {
        Remove-Item -Path $temporaryKeyFile -Force
    }
}

Write-Host 'Gemini API Key 和模型配置已更新，服务运行正常。'
