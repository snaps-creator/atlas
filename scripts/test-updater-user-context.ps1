param([Parameter(Mandatory=$true)][string]$Updater, [Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference = 'Stop'
$executable = (Resolve-Path -LiteralPath $Updater).Path
$output = Join-Path $OutputDirectory 'user-context.json'
$errors = Join-Path $OutputDirectory 'user-context.err'
try {
    $child = Start-Process -FilePath $executable -ArgumentList '--check-user-context' -WindowStyle Hidden -PassThru -RedirectStandardOutput $output -RedirectStandardError $errors
    $nativeHandle = $child.Handle
    if (-not $child.WaitForExit(30000)) { $child.Kill(); throw 'User-context native test timed out' }
    $child.WaitForExit()
    $code = $child.ExitCode
    if ($null -eq $code) { throw 'Native user-context process exit code is unavailable' }
} catch {
    $_ | Out-String | Set-Content -LiteralPath $errors -Encoding utf8
    $code = 1
}
[ordered]@{ exitCode = $code; completedAt = [DateTime]::UtcNow.ToString('o') } |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $OutputDirectory 'user-context-result.json') -Encoding utf8
exit $code
