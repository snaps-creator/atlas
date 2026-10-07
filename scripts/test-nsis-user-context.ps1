param([Parameter(Mandatory=$true)][string]$Updater,[Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference='Stop'
$source=(Resolve-Path -LiteralPath $Updater).Path
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$output=(Resolve-Path -LiteralPath $OutputDirectory).Path
$stage=Join-Path $output ('staged-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
# Same protected permissions as the production installation directory.
$acl=[Security.AccessControl.DirectorySecurity]::new()
$acl.SetSecurityDescriptorSddlForm('O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;GRGX;;;AU)')
Set-Acl -LiteralPath $stage -AclObject $acl
Copy-Item -LiteralPath $source -Destination (Join-Path $stage 'AtlasUpdater.exe')
$nsis=@'
Unicode true
!include LogicLib.nsh
Name "Atlas user context regression"
OutFile "@OUTPUT@\context-regression.exe"
RequestExecutionLevel admin
SilentInstall silent
Section
  InitPluginsDir
  SetOutPath "$PLUGINSDIR\payload"
  File /oname=AtlasUpdater.exe "@SOURCE@"
  FileOpen $2 "@OUTPUT@\result.txt" w
  nsExec::ExecToStack '"$PLUGINSDIR\payload\AtlasUpdater.exe" --check-user-context'
  Pop $0
  Pop $1
  FileWrite $2 "private_exit=$0$\r$\n$1$\r$\n"
  ${If} $0 == 0
    FileClose $2
    SetErrorLevel 10
    Quit
  ${EndIf}
  nsExec::ExecToStack '"$PLUGINSDIR\payload\AtlasUpdater.exe" --check-user-context "@STAGE@\AtlasUpdater.exe"'
  Pop $0
  Pop $1
  FileWrite $2 "staged_exit=$0$\r$\n$1$\r$\n"
  FileClose $2
  ${If} $0 != 0
    SetErrorLevel 11
    Quit
  ${EndIf}
  SetErrorLevel 0
SectionEnd
'@
function Literal([string]$value) { $value.Replace('$','$$').Replace('"','$\"') }
$nsis=$nsis.Replace('@OUTPUT@',(Literal $output)).Replace('@SOURCE@',(Literal $source)).Replace('@STAGE@',(Literal $stage))
$script=Join-Path $output 'context-regression.nsi'
[IO.File]::WriteAllText($script,$nsis,[Text.UTF8Encoding]::new($true))
& 'C:/Users/Никита/AppData/Local/tauri/NSIS/makensis.exe' /V2 $script *> (Join-Path $output 'build.log')
if ($LASTEXITCODE -ne 0) { throw 'NSIS regression build failed' }
$process=Start-Process -FilePath (Join-Path $output 'context-regression.exe') -PassThru -WindowStyle Hidden
$handle=$process.Handle
if (-not $process.WaitForExit(60000)) { throw 'NSIS regression did not finish' }
$process.WaitForExit()
[ordered]@{exitCode=$process.ExitCode;completedAt=[DateTime]::UtcNow.ToString('o');stage=$stage;networkTouched=$false} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $output 'result.json') -Encoding utf8
exit $process.ExitCode
