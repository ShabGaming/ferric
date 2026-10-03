!macro NSIS_HOOK_POSTUNINSTALL
  ; I remove startup only when it still points to this installed copy.
  Push $0
  ReadRegStr $0 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Ferric"
  ${If} $0 == '$\"$INSTDIR\${MAINBINARYNAME}.exe$\" --startup'
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Ferric"
  ${EndIf}
  Pop $0
!macroend
