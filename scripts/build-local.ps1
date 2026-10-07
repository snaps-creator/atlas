# Local builds use the same transactional package and checks as release CI.
param([switch]$Signed)
& (Join-Path $PSScriptRoot 'build.ps1') -Signed:$Signed
