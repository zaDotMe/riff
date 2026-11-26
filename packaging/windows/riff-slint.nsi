; Riff Slint Windows NSIS Installer
!include "MUI2.nsh"
!include "FileFunc.nsh"

!ifndef APP_VERSION
  !define APP_VERSION "0.7.6"
!endif

!define APP_NAME "Riff"
!define APP_PUBLISHER "zaDotMe"
!define APP_EXE "riff-slint.exe"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Riff"

Name "${APP_NAME} ${APP_VERSION}"
OutFile "..\..\dist-release\Riff-Setup-windows-x64.exe"
InstallDir "$LOCALAPPDATA\Programs\Riff"
InstallDirRegKey HKCU "Software\Riff" "InstallDir"
RequestExecutionLevel user
Unicode true
SetCompressor /SOLID lzma

!define MUI_ICON "..\..\src-tauri\icons\icon.ico"
!define MUI_UNICON "..\..\src-tauri\icons\icon.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Launch Riff"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Riff (Slint Native)" SecMain
  SetOutPath "$INSTDIR"

  File "..\..\target\release\riff-slint.exe"
  File "..\..\target\release\riff-core.exe"
  File "..\..\src-tauri\icons\icon.ico"

  WriteRegStr HKCU "Software\Riff" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  ; Start Menu & Desktop Shortcuts
  CreateDirectory "$SMPROGRAMS\Riff"
  CreateShortcut "$SMPROGRAMS\Riff\Riff.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\icon.ico"
  CreateShortcut "$SMPROGRAMS\Riff\Uninstall Riff.lnk" "$INSTDIR\Uninstall.exe"
  CreateShortcut "$DESKTOP\Riff.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\icon.ico"

  ; Add/Remove Programs Entry
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\icon.ico"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" "$0"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\riff-slint.exe"
  Delete "$INSTDIR\riff-core.exe"
  Delete "$INSTDIR\icon.ico"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$DESKTOP\Riff.lnk"
  Delete "$SMPROGRAMS\Riff\Riff.lnk"
  Delete "$SMPROGRAMS\Riff\Uninstall Riff.lnk"
  RMDir "$SMPROGRAMS\Riff"

  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey HKCU "Software\Riff"
SectionEnd
