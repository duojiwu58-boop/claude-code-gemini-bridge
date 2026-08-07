param(
    [string]$Version = '0.4.0'
)

$ErrorActionPreference = 'Stop'

$projectDir = Split-Path -Parent $PSScriptRoot
$packageTemplateDir = Join-Path $projectDir 'packaging\windows-x64'
$buildTargetDir = Join-Path $projectDir 'target\package-build'
$builtExe = Join-Path $buildTargetDir 'x86_64-pc-windows-msvc\release\claude-bridge.exe'
$guiExe = Join-Path $projectDir 'ClaudeBridgeManager.exe'
$appIcon = Join-Path `
    $projectDir `
    'gui\delphi11\ClaudeBridgeManager\assets\ClaudeBridgeManager.ico'
$settingsTemplate = Join-Path $projectDir 'claude-settings.example.json'
$bridgeSettingsTemplate = Join-Path $projectDir 'claude-settings.bridge.json'
$providerGuide = Join-Path $projectDir 'PROVIDER_CONFIG.md'
$providerExamples = Join-Path $projectDir 'examples\providers'
$licenseFile = Join-Path $projectDir 'LICENSE'
$innoScript = Join-Path $projectDir 'packaging\inno\ClaudeCodeBridge.iss'
$distDir = Join-Path $projectDir 'dist'
$packageName = "ClaudeCodeBridge-$Version-windows-x64"
$stagingDir = Join-Path $distDir $packageName
$zipPath = Join-Path $distDir "$packageName.zip"
$setupPath = Join-Path $distDir "ClaudeCodeBridge-$Version-Setup.exe"
$releaseChecksumsPath = Join-Path $distDir "SHA256SUMS-v$Version.txt"

foreach ($requiredPath in @(
    $packageTemplateDir
    $guiExe
    $appIcon
    $settingsTemplate
    $bridgeSettingsTemplate
    $providerGuide
    $providerExamples
    $licenseFile
    $innoScript
)) {
    if (-not (Test-Path -LiteralPath $requiredPath)) {
        throw "Required release input does not exist: $requiredPath"
    }
}

$resolvedProject = [System.IO.Path]::GetFullPath($projectDir)
$resolvedDist = [System.IO.Path]::GetFullPath($distDir)
$cargoHome = if ([string]::IsNullOrWhiteSpace($env:CARGO_HOME)) {
    Join-Path $env:USERPROFILE '.cargo'
} else {
    $env:CARGO_HOME
}
$resolvedCargoHome = [System.IO.Path]::GetFullPath($cargoHome)
if (-not $resolvedDist.StartsWith(
    $resolvedProject + [System.IO.Path]::DirectorySeparatorChar,
    [StringComparison]::OrdinalIgnoreCase
)) {
    throw "Refusing to manage a dist directory outside the project: $resolvedDist"
}

$env:CARGO_TARGET_DIR = $buildTargetDir
$env:CARGO_ENCODED_RUSTFLAGS = @(
    '-C'
    'target-feature=+crt-static'
    "--remap-path-prefix=$resolvedProject=bridge"
    "--remap-path-prefix=$resolvedCargoHome=rust-cargo"
) -join [char]0x1F
Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
& cargo build --locked --release --target x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) {
    throw "Cargo package build failed with exit code $LASTEXITCODE."
}
if (-not (Test-Path -LiteralPath $builtExe -PathType Leaf)) {
    throw "Package executable was not produced: $builtExe"
}

New-Item -ItemType Directory -Path $distDir -Force | Out-Null
if (Test-Path -LiteralPath $stagingDir) {
    $resolvedStaging = [System.IO.Path]::GetFullPath($stagingDir)
    if (-not $resolvedStaging.StartsWith(
        $resolvedDist + [System.IO.Path]::DirectorySeparatorChar,
        [StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Refusing to replace unexpected staging directory: $resolvedStaging"
    }
    Remove-Item -LiteralPath $resolvedStaging -Recurse -Force
}
if (Test-Path -LiteralPath $zipPath -PathType Leaf) {
    Remove-Item -LiteralPath $zipPath -Force
}

Copy-Item -LiteralPath $packageTemplateDir -Destination $stagingDir -Recurse
Copy-Item `
    -LiteralPath $builtExe `
    -Destination (Join-Path $stagingDir 'claude-bridge.exe')
Copy-Item `
    -LiteralPath $guiExe `
    -Destination (Join-Path $stagingDir 'ClaudeBridgeManager.exe')
Copy-Item `
    -LiteralPath $appIcon `
    -Destination (Join-Path $stagingDir 'ClaudeBridgeManager.ico')
Copy-Item `
    -LiteralPath $settingsTemplate `
    -Destination (Join-Path $stagingDir 'claude-settings.example.json')
Copy-Item `
    -LiteralPath $bridgeSettingsTemplate `
    -Destination (Join-Path $stagingDir 'claude-settings.bridge.json')
Copy-Item `
    -LiteralPath $providerGuide `
    -Destination (Join-Path $stagingDir 'PROVIDER_CONFIG.md')
New-Item `
    -ItemType Directory `
    -Path (Join-Path $stagingDir 'examples') `
    -Force | Out-Null
Copy-Item `
    -LiteralPath $providerExamples `
    -Destination (Join-Path $stagingDir 'examples\providers') `
    -Recurse
Copy-Item `
    -LiteralPath $licenseFile `
    -Destination (Join-Path $stagingDir 'LICENSE')

$usagePath = Join-Path $stagingDir '使用说明.txt'
$usageText = [System.IO.File]::ReadAllText(
    $usagePath,
    [System.Text.UTF8Encoding]::new($false)
)
[System.IO.File]::WriteAllText(
    $usagePath,
    $usageText,
    [System.Text.UTF8Encoding]::new($true)
)

$hashLines = Get-ChildItem -LiteralPath $stagingDir -File -Recurse |
    Sort-Object FullName |
    ForEach-Object {
        $relativePath = $_.FullName.Substring($stagingDir.Length + 1)
        $hash = Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256
        "$($hash.Hash)  $relativePath"
    }
[System.IO.File]::WriteAllLines(
    (Join-Path $stagingDir 'SHA256SUMS.txt'),
    $hashLines,
    [System.Text.UTF8Encoding]::new($false)
)

Compress-Archive -LiteralPath $stagingDir -DestinationPath $zipPath -CompressionLevel Optimal

$innoCandidates = @(
    (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe')
    (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
    'E:\Program Files (x86)\Inno Setup 5\ISCC.exe'
)
$iscc = $innoCandidates |
    Where-Object {
        -not [string]::IsNullOrWhiteSpace($_) -and
        (Test-Path -LiteralPath $_ -PathType Leaf)
    } |
    Select-Object -First 1
if ([string]::IsNullOrWhiteSpace($iscc)) {
    throw '找不到 Inno Setup 命令行编译器 ISCC.exe。'
}
if (Test-Path -LiteralPath $setupPath -PathType Leaf) {
    Remove-Item -LiteralPath $setupPath -Force
}
$innoCompileScript = $innoScript
$innoMajorVersion = (Get-Item -LiteralPath $iscc).VersionInfo.ProductMajorPart
if ($innoMajorVersion -lt 6) {
    # Inno Setup 5 ANSI cannot parse a UTF-8 BOM script. Keep the repository
    # source in UTF-8 and create a CP936 compiler copy for this legacy build.
    $innoBuildDir = Join-Path $buildTargetDir 'inno'
    New-Item -ItemType Directory -Path $innoBuildDir -Force | Out-Null
    $innoCompileScript = Join-Path $innoBuildDir 'ClaudeCodeBridge.iss'
    $innoText = [System.IO.File]::ReadAllText(
        $innoScript,
        [System.Text.UTF8Encoding]::new($true)
    )
    [System.IO.File]::WriteAllText(
        $innoCompileScript,
        $innoText,
        [System.Text.Encoding]::GetEncoding(936)
    )
}
& $iscc `
    "/DAppVersion=$Version" `
    "/DSourceDir=$stagingDir" `
    "/DOutputDir=$distDir" `
    $innoCompileScript
if ($LASTEXITCODE -ne 0) {
    throw "Inno Setup 编译失败，ISCC.exe 返回 $LASTEXITCODE。"
}
if (-not (Test-Path -LiteralPath $setupPath -PathType Leaf)) {
    throw "Inno Setup 未生成预期安装包：$setupPath"
}

$zipHash = Get-FileHash -LiteralPath $zipPath -Algorithm SHA256
$setupHash = Get-FileHash -LiteralPath $setupPath -Algorithm SHA256
[System.IO.File]::WriteAllLines(
    $releaseChecksumsPath,
    @(
        "$($setupHash.Hash)  $($setupPath | Split-Path -Leaf)"
        "$($zipHash.Hash)  $($zipPath | Split-Path -Leaf)"
    ),
    [System.Text.UTF8Encoding]::new($false)
)
Write-Output "package_dir=$stagingDir"
Write-Output "package_zip=$zipPath"
Write-Output "zip_sha256=$($zipHash.Hash)"
Write-Output "package_setup=$setupPath"
Write-Output "setup_sha256=$($setupHash.Hash)"
Write-Output "release_checksums=$releaseChecksumsPath"
