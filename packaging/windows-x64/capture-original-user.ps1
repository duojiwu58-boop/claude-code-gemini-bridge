param(
    [Parameter(Mandatory)]
    [string]$OutputPath
)

$ErrorActionPreference = 'Stop'

function Test-StringNullOrWhitespace {
    param([string]$Value)
    if ([string]::IsNullOrEmpty($Value)) {
        return $true
    }
    return $Value.Trim().Length -eq 0
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$values = @(
    [System.IO.Path]::GetFullPath($env:USERPROFILE)
    [Environment]::GetFolderPath([Environment+SpecialFolder]::MyPictures)
    [Environment]::GetFolderPath('DesktopDirectory')
    $identity.User.Value
)
if ($values.Count -ne 4 -or $values | Where-Object { Test-StringNullOrWhitespace ($_) }) {
    throw 'Cannot resolve the original Windows user profile, shell folders, or SID.'
}

$resolvedOutput = [System.IO.Path]::GetFullPath($OutputPath)
$temporaryOutput = "$resolvedOutput.tmp-$PID-$([Guid]::NewGuid().ToString('N'))"
try {
    [System.IO.File]::WriteAllLines(
        $temporaryOutput,
        $values,
        [System.Text.Encoding]::Unicode
    )
    [System.IO.File]::Move($temporaryOutput, $resolvedOutput)
}
finally {
    if ([System.IO.File]::Exists($temporaryOutput)) {
        [System.IO.File]::Delete($temporaryOutput)
    }
}
