; Overdrive installer hooks (Tauri NSIS, per-machine install = elevated).
; The service is what changes the system; the app is only a window + tray.

!macro NSIS_HOOK_PREINSTALL
  ; Upgrade: stop the running service so its files can be replaced.
  nsExec::ExecToLog 'sc.exe stop OverdriveSvc'
  Sleep 1500
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Register + start the LocalSystem service and lock its data folder.
  nsExec::ExecToLog '"$INSTDIR\overdrive-svc.exe" install'
  ; Tray app at sign-in for every user.
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Run" "Overdrive" '"$INSTDIR\overdrive.exe" --tray'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Put the PC back exactly as it was, then remove the service.
  nsExec::ExecToLog '"$INSTDIR\overdrive-svc.exe" uninstall'
  DeleteRegValue HKLM "Software\Microsoft\Windows\CurrentVersion\Run" "Overdrive"
!macroend
