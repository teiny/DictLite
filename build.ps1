param([switch]$DebugBuild)
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot
$env:CARGO_HOME = Join-Path $PSScriptRoot 'work\cargo'
$taskProfile = if ($DebugBuild) { 'debug' } else { 'release' }
if ($DebugBuild) { cargo build --bins } else { cargo build --release --bins }
if ($LASTEXITCODE -ne 0) { throw 'DictLite 编译失败。' }
$taskOutput = Join-Path $PSScriptRoot 'dist\DictLite'
New-Item -ItemType Directory -Path $taskOutput -Force | Out-Null
foreach ($taskBinary in @('dictlite.exe','dictlite-convert.exe')) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot "target\$taskProfile\$taskBinary") -Destination $taskOutput
}
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'README.md') -Destination $taskOutput
Write-Output "便携程序已输出：$taskOutput"
