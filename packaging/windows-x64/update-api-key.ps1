$ErrorActionPreference = 'Stop'

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator
    )
}

if (-not (Test-IsAdministrator)) {
    $elevationArguments = (
        '-NoProfile -ExecutionPolicy Bypass -File "{0}"' -f $PSCommandPath
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

$installScript = Join-Path (Split-Path -Parent $PSCommandPath) 'install.ps1'
if (-not (Test-Path -LiteralPath $installScript -PathType Leaf)) {
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
if ([string]::IsNullOrWhiteSpace($apiKey)) {
    throw 'API Key 不能为空。'
}

$temporaryKeyFile = Join-Path (
    [System.IO.Path]::GetTempPath()
) "claude-bridge-key-$([Guid]::NewGuid().ToString('N')).txt"
try {
    [System.IO.File]::WriteAllText(
        $temporaryKeyFile,
        $apiKey,
        [System.Text.UTF8Encoding]::new($false)
    )
    $apiKey = $null
    & $installScript `
        -GeminiMode Configure `
        -ApiKeyFile $temporaryKeyFile `
        -ClaudeSettingsDir (Join-Path $env:USERPROFILE '.claude')
}
finally {
    $apiKey = $null
    if (Test-Path -LiteralPath $temporaryKeyFile -PathType Leaf) {
        Remove-Item -LiteralPath $temporaryKeyFile -Force
    }
}

Write-Host 'Gemini API Key 和模型配置已更新，服务运行正常。'
