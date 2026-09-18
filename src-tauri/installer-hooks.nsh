!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\atlas-vpn.exe" --cleanup' $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось восстановить сеть. Откройте Атлас, отключите VPN и повторите удаление."
    Abort
  ${EndIf}
!macroend
