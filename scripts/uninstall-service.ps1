param(
    [switch]$RemoveServiceFiles
)

$ErrorActionPreference = 'Stop'

$serviceName = 'ClaudeCodeBridge'
$projectDir = Split-Path -Parent $PSScriptRoot
$serviceDir = Join-Path $projectDir 'service'

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Uninstalling the Windows service requires an elevated PowerShell process.'
}

$service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($null -eq $service) {
    Write-Output 'service_status=not_installed'
    exit 0
}

if ($service.Status -ne 'Stopped') {
    Stop-Service -Name $serviceName -Force
    $service.WaitForStatus(
        [System.ServiceProcess.ServiceControllerStatus]::Stopped,
        [TimeSpan]::FromSeconds(30)
    )
}

& sc.exe delete $serviceName | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Failed to delete service (sc.exe exit $LASTEXITCODE)."
}

if ($RemoveServiceFiles -and (Test-Path -LiteralPath $serviceDir -PathType Container)) {
    $resolvedProject = [System.IO.Path]::GetFullPath($projectDir)
    $resolvedService = [System.IO.Path]::GetFullPath($serviceDir)
    if (-not $resolvedService.StartsWith(
        $resolvedProject + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Refusing to remove service files outside the project: $resolvedService"
    }
    Remove-Item -LiteralPath $resolvedService -Recurse -Force
    Write-Output 'service_files=removed'
}

Write-Output 'service_status=uninstalled'
