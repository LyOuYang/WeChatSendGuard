Unicode True
ManifestDPIAware true

!ifndef APP_VERSION
!error "APP_VERSION must be supplied by the release script."
!endif

!ifndef APP_VERSION_WIN
!error "APP_VERSION_WIN must be supplied by the release script."
!endif

!ifndef APP_EXECUTABLE
!error "APP_EXECUTABLE must be supplied by the release script."
!endif

!ifndef OUTPUT_FILE
!error "OUTPUT_FILE must be supplied by the release script."
!endif

!include "MUI2.nsh"

Name "WeChatSendGuard ${APP_VERSION}"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\WeChatSendGuard"
InstallDirRegKey HKCU "Software\WeChatSendGuard" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
SetDatablockOptimize on
ShowInstDetails show
ShowUninstDetails show

VIProductVersion "${APP_VERSION_WIN}"
VIAddVersionKey /LANG=2052 "ProductName" "WeChatSendGuard"
VIAddVersionKey /LANG=2052 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=2052 "FileDescription" "WeChatSendGuard 安装程序"
VIAddVersionKey /LANG=2052 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=2052 "CompanyName" "WeChatSendGuard"
VIAddVersionKey /LANG=2052 "LegalCopyright" "Copyright (c) WeChatSendGuard contributors"
VIAddVersionKey /LANG=2052 "OriginalFilename" "WeChatSendGuard-Setup-${APP_VERSION}.exe"

!define MUI_ICON "..\..\crates\desktop-ui\assets\app.ico"
!define MUI_UNICON "..\..\crates\desktop-ui\assets\app.ico"
!define MUI_ABORTWARNING
!define MUI_STARTMENUPAGE_DEFAULTFOLDER "WeChatSendGuard"
!define MUI_STARTMENUPAGE_REGISTRY_ROOT "HKCU"
!define MUI_STARTMENUPAGE_REGISTRY_KEY "Software\WeChatSendGuard"
!define MUI_STARTMENUPAGE_REGISTRY_VALUENAME "StartMenuFolder"
!define MUI_FINISHPAGE_RUN "$INSTDIR\WeChatSendGuard.exe"
!define MUI_FINISHPAGE_RUN_TEXT "启动 WeChatSendGuard"

Var StartMenuFolder

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_STARTMENU Application $StartMenuFolder
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Section "安装"
    SetShellVarContext current
    ExecWait 'taskkill /F /IM WeChatSendGuard.exe' $0
    Sleep 500
    SetOutPath "$INSTDIR"
    File /oname=WeChatSendGuard.exe "${APP_EXECUTABLE}"
    WriteUninstaller "$INSTDIR\Uninstall.exe"

    !insertmacro MUI_STARTMENU_WRITE_BEGIN Application
        CreateDirectory "$SMPROGRAMS\$StartMenuFolder"
        CreateShortcut "$SMPROGRAMS\$StartMenuFolder\WeChatSendGuard.lnk" "$INSTDIR\WeChatSendGuard.exe"
        CreateShortcut "$SMPROGRAMS\$StartMenuFolder\卸载 WeChatSendGuard.lnk" "$INSTDIR\Uninstall.exe"
    !insertmacro MUI_STARTMENU_WRITE_END

    WriteRegStr HKCU "Software\WeChatSendGuard" "InstallDir" "$INSTDIR"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WeChatSendGuard" "DisplayName" "WeChatSendGuard ${APP_VERSION}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WeChatSendGuard" "DisplayVersion" "${APP_VERSION}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WeChatSendGuard" "Publisher" "WeChatSendGuard"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WeChatSendGuard" "DisplayIcon" "$INSTDIR\WeChatSendGuard.exe"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WeChatSendGuard" "UninstallString" '"$INSTDIR\Uninstall.exe"'
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WeChatSendGuard" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WeChatSendGuard" "NoModify" 1
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WeChatSendGuard" "NoRepair" 1
    System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
SectionEnd

Section "Uninstall"
    SetShellVarContext current
    ExecWait 'taskkill /F /IM WeChatSendGuard.exe' $0
    Sleep 500
    !insertmacro MUI_STARTMENU_GETFOLDER Application $StartMenuFolder
    Delete "$SMPROGRAMS\$StartMenuFolder\WeChatSendGuard.lnk"
    Delete "$SMPROGRAMS\$StartMenuFolder\卸载 WeChatSendGuard.lnk"
    RMDir "$SMPROGRAMS\$StartMenuFolder"

    Delete "$INSTDIR\WeChatSendGuard.exe"
    Delete "$INSTDIR\Uninstall.exe"
    RMDir "$INSTDIR"
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WeChatSendGuard"
    DeleteRegKey HKCU "Software\WeChatSendGuard"
    System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
SectionEnd
