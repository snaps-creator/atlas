; Double-click upgrades must take the same in-place path as the updater. The
; stock reinstall page otherwise launches the OLD uninstaller before our hook.
!define MUI_CUSTOMFUNCTION_GUIINIT AtlasUpgradeEntry
Function AtlasUpgradeEntry
  ${GetOptions} $CMDLINE "/UPDATE" $0
  ${IfNot} ${Errors}
    Return
  ${EndIf}
  IfFileExists "$INSTDIR\Atlas.exe" 0 atlas_entry_done
    ExecWait '"$EXEPATH" /P /UPDATE /R /D=$INSTDIR' $0
    ${If} ${Errors}
      MessageBox MB_OK|MB_ICONSTOP "Не удалось запустить установку обновления Atlas." /SD IDOK
      SetErrorLevel 1
      Quit
    ${EndIf}
    SetErrorLevel $0
    Quit
  atlas_entry_done:
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
  ; Extract NEW native recovery before replacing any installed files. Never
  ; execute the old app's broken cleanup (and never require CEF for recovery).
  InitPluginsDir
  !searchreplace ATLAS_MAINTENANCE_BINARY "${MAINBINARYSRCPATH}" "${MAINBINARYNAME}.exe" "AtlasMaintenance.exe"
  File /oname=$PLUGINSDIR\AtlasMaintenance.exe "${ATLAS_MAINTENANCE_BINARY}"
  nsExec::ExecToStack '"$PLUGINSDIR\AtlasMaintenance.exe" --prepare-install "$INSTDIR"'
  Pop $0
  Pop $1
  ; Silent installs cannot show the helper's stderr. Preserve the original
  ; result before any later recovery attempt changes the state.
  FileOpen $2 "$TEMP\atlas-install-recovery.log" a
  FileWrite $2 "prepare-install: $INSTDIR$\r$\nexit: $0$\r$\n$1$\r$\n"
  FileClose $2
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось завершить сетевую сессию Atlas. Файлы не заменены.$\r$\n$1" /SD IDOK
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  CopyFiles /SILENT "$PLUGINSDIR\AtlasMaintenance.exe" "$INSTDIR\AtlasMaintenance.exe"
  ; Tauri's CEF bundle contains upstream bootstrap hosts for a DLL-hosted
  ; application. Atlas is an EXE-hosted application and never launches them.
  ; Remove these unused foreign-named executables after extraction.
  Delete "$INSTDIR\bootstrap.exe"
  Delete "$INSTDIR\bootstrapc.exe"
  CopyFiles /SILENT "$INSTDIR\Atlas.exe" "$INSTDIR\Atlas.Service.exe"
  ExecWait '"$INSTDIR\Atlas.exe" --install-service' $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось установить сетевую службу Atlas." /SD IDOK
    Abort
  ${EndIf}
  nsExec::ExecToLog 'sc.exe failure AtlasNetworkService reset= 0 actions= ""'
  Pop $0
  nsExec::ExecToLog 'sc.exe failureflag AtlasNetworkService 0'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  InitPluginsDir
  CopyFiles /SILENT "$INSTDIR\AtlasMaintenance.exe" "$PLUGINSDIR\AtlasMaintenance.exe"
  nsExec::ExecToStack '"$PLUGINSDIR\AtlasMaintenance.exe" --prepare-install "$INSTDIR"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось восстановить сеть. Удаление отменено.$\r$\n$1" /SD IDOK
    Abort
  ${EndIf}
  ExecWait '"$INSTDIR\Atlas.exe" --uninstall-service' $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Служба Atlas не удалена. Удаление файлов остановлено, чтобы сохранить возможность восстановления." /SD IDOK
    Abort
  ${EndIf}
  Delete "$INSTDIR\Atlas.Service.exe"
  Delete "$INSTDIR\AtlasMaintenance.exe"
!macroend
; Tauri's default macro kills by executable NAME across all directories. The
; preinstall/preuninstall helper below already stops exact verified paths.
!macroundef CheckIfAppIsRunning
!macro CheckIfAppIsRunning executableName productName
!macroend
