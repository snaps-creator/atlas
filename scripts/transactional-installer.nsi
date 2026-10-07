Unicode true
!include MUI2.nsh
!include LogicLib.nsh
Name "Atlas"
OutFile "@OUTPUT@"
RequestExecutionLevel admin
InstallDir "$PROGRAMFILES64\Atlas"
InstallDirRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Atlas" "InstallLocation"
SetCompressor /SOLID lzma
VIProductVersion "@VERSION@.0"
VIAddVersionKey /LANG=1033 "ProductName" "Atlas"
VIAddVersionKey /LANG=1033 "ProductVersion" "@VERSION@"
VIAddVersionKey /LANG=1033 "FileVersion" "@VERSION@"
VIAddVersionKey /LANG=1033 "FileDescription" "Atlas Setup"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Atlas"
!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "Russian"
!insertmacro MUI_LANGUAGE "English"
Function .onInit
  SetRegView 64
  SetShellVarContext all
FunctionEnd
Function un.onInit
  SetRegView 64
  SetShellVarContext all
FunctionEnd
Section "Atlas"
  InitPluginsDir
  SetOutPath "$PLUGINSDIR\payload"
  File /r "@PAYLOAD@\*"
  nsExec::ExecToStack '"$PLUGINSDIR\payload\AtlasUpdater.exe" --install-local "$INSTDIR" "$PLUGINSDIR\payload" "$EXEPATH"'
  Pop $0
  Pop $1
  FileOpen $2 "$TEMP\atlas-transactional-install.log" a
  FileSeek $2 0 END
  FileWrite $2 "exit=$0$\r$\n$1$\r$\n"
  FileClose $2
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось завершить установку Atlas.$\r$\n$1$\r$\nПодробности: $TEMP\atlas-transactional-install.log" /SD IDOK
    SetErrorLevel 5
    Abort
  ${EndIf}
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Atlas" "DisplayName" "Atlas"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Atlas" "DisplayVersion" "@VERSION@"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Atlas" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Atlas" "UninstallString" '$\"$INSTDIR\uninstall.exe$\"'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Atlas" "QuietUninstallString" '$\"$INSTDIR\uninstall.exe$\" /S'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Atlas" "DisplayIcon" "$INSTDIR\Atlas.exe"
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Atlas" "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Atlas" "NoRepair" 1
  CreateDirectory "$SMPROGRAMS\Atlas"
  CreateShortCut "$SMPROGRAMS\Atlas\Atlas.lnk" "$INSTDIR\Atlas.exe"
  CreateShortCut "$DESKTOP\Atlas.lnk" "$INSTDIR\Atlas.exe"
SectionEnd
Section "Uninstall"
  InitPluginsDir
  CopyFiles /SILENT "$INSTDIR\AtlasUpdater.exe" "$PLUGINSDIR\AtlasUpdater.exe"
  nsExec::ExecToStack '"$PLUGINSDIR\AtlasUpdater.exe" --uninstall-local "$INSTDIR"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Не удалось завершить удаление Atlas.$\r$\n$1" /SD IDOK
    SetErrorLevel 6
    Abort
  ${EndIf}
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$DESKTOP\Atlas.lnk"
  Delete "$SMPROGRAMS\Atlas\Atlas.lnk"
  RMDir "$SMPROGRAMS\Atlas"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Atlas"
SectionEnd
