; NSIS hooks for the Tauri installer.
; On install, adds $INSTDIR to the current user's PATH.
; On uninstall, removes $INSTDIR from the current user's PATH.
; PowerShell does the edit, so long PATH values are not cut at NSIS's string limit.

!macro NSIS_HOOK_POSTINSTALL
  ; Append the install folder unless an entry already matches it.
  nsExec::ExecToLog `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$d = '$INSTDIR'.TrimEnd('\'); $$p = [Environment]::GetEnvironmentVariable('Path', 'User'); $$e = @(); if ($$p) { $$e = @($$p -split ';' | Where-Object { $$_ -ne '' }) }; if (-not ($$e | Where-Object { $$_.TrimEnd('\') -ieq $$d })) { [Environment]::SetEnvironmentVariable('Path', (($$e + $$d) -join ';'), 'User') }"`
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Drop every entry that matches the install folder. Other entries stay as they are.
  nsExec::ExecToLog `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$d = '$INSTDIR'.TrimEnd('\'); $$p = [Environment]::GetEnvironmentVariable('Path', 'User'); if ($$p) { $$k = @($$p -split ';' | Where-Object { $$_ -ne '' -and $$_.TrimEnd('\') -ine $$d }); [Environment]::SetEnvironmentVariable('Path', ($$k -join ';'), 'User') }"`
  Pop $0
!macroend
