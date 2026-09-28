!macro NSIS_HOOK_PREINSTALL
  ; An update must use the same verified cleanup as Disconnect and Uninstall
  ; before replacing either the core or the service executable.
  IfFileExists "$INSTDIR\Atlas.exe" 0 atlas_preinstall_clean
    ExecWait '"$INSTDIR\Atlas.exe" --cleanup' $0
    ${If} $0 != 0
      MessageBox MB_OK|MB_ICONSTOP "Не удалось завершить прежнюю сетевую сессию Atlas. Обновление отменено."
      Abort
    ${EndIf}
  atlas_preinstall_clean:
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Recheck with the NEW cleanup implementation as older releases did not
  ; verify that Atlas-TUN actually disappeared. Do this before registration.
  ExecWait '"$INSTDIR\Atlas.exe" --cleanup' $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Предыдущая сетевая сессия Atlas не очищена полностью. Новая служба не будет зарегистрирована."
    Abort
  ${EndIf}
  ; Tauri's CEF bundle contains upstream bootstrap hosts for a DLL-hosted
  ; application. Atlas is an EXE-hosted application and never launches them.
  ; Remove these unused foreign-named executables after extraction.
  Delete "$INSTDIR\bootstrap.exe"
  Delete "$INSTDIR\bootstrapc.exe"
  CopyFiles /SILENT "$INSTDIR\Atlas.exe" "$INSTDIR\Atlas.Service.exe"
  ExecWait '"$INSTDIR\Atlas.exe" --install-service' $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось установить сетевую службу Atlas."
    Abort
  ${EndIf}
  nsExec::ExecToLog 'sc.exe failure AtlasNetworkService reset= 0 actions= ""'
  Pop $0
  nsExec::ExecToLog 'sc.exe failureflag AtlasNetworkService 0'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\Atlas.exe" --cleanup' $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось восстановить сеть. Откройте Атлас, отключите VPN и повторите удаление."
    Abort
  ${EndIf}
  ExecWait '"$INSTDIR\Atlas.exe" --uninstall-service' $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Служба Atlas не удалена. Удаление файлов остановлено, чтобы сохранить возможность восстановления."
    Abort
  ${EndIf}
  Delete "$INSTDIR\Atlas.Service.exe"
!macroend
