!macro NSIS_HOOK_PREINSTALL
  nsExec::ExecToLog 'sc.exe stop AtlasNetworkService'
  Pop $0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog 'sc.exe create AtlasNetworkService binPath= "$\"$INSTDIR\atlas-vpn.exe$\" --network-service" start= auto DisplayName= "Atlas Network Service"'
  Pop $0
  ${If} $0 != 0
    nsExec::ExecToLog 'sc.exe config AtlasNetworkService binPath= "$\"$INSTDIR\atlas-vpn.exe$\" --network-service" start= auto DisplayName= "Atlas Network Service"'
    Pop $0
  ${EndIf}
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось установить сетевую службу Atlas."
    Abort
  ${EndIf}
  nsExec::ExecToLog 'sc.exe failure AtlasNetworkService reset= 86400 actions= restart/5000/restart/15000/restart/30000'
  Pop $0
  nsExec::ExecToLog 'sc.exe failureflag AtlasNetworkService 1'
  Pop $0
  nsExec::ExecToLog 'sc.exe start AtlasNetworkService'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось запустить сетевую службу Atlas."
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\atlas-vpn.exe" --cleanup' $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось восстановить сеть. Откройте Атлас, отключите VPN и повторите удаление."
    Abort
  ${EndIf}
  nsExec::ExecToLog 'sc.exe stop AtlasNetworkService'
  Pop $0
  nsExec::ExecToLog 'sc.exe delete AtlasNetworkService'
  Pop $0
!macroend
