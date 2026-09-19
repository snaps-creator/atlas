!macro NSIS_HOOK_PREINSTALL
  nsExec::ExecToLog 'sc.exe stop AtlasNetworkService'
  Pop $0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ExecWait '"$INSTDIR\atlas-vpn.exe" --install-service' $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось установить сетевую службу Atlas."
    Abort
  ${EndIf}
  nsExec::ExecToLog 'sc.exe failure AtlasNetworkService reset= 86400 actions= restart/5000/restart/15000/restart/30000'
  Pop $0
  nsExec::ExecToLog 'sc.exe failureflag AtlasNetworkService 1'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\atlas-vpn.exe" --cleanup' $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось восстановить сеть. Откройте Атлас, отключите VPN и повторите удаление."
    Abort
  ${EndIf}
  ExecWait '"$INSTDIR\atlas-vpn.exe" --uninstall-service' $0
!macroend
