; NSIS hooks for the Tauri installer.
; On install, adds $INSTDIR to the current user's PATH.
; On uninstall, removes $INSTDIR from the current user's PATH.
; PowerShell edits the raw registry value, so other entries and %VARS% are kept as they are.

!macro NSIS_HOOK_POSTINSTALL
  ; Pass the folder through the environment so quotes in the path cannot break the command.
  System::Call 'Kernel32::SetEnvironmentVariable(t "LIBREPAPER_INSTDIR", t "$INSTDIR")i'
  ; Append the install folder unless an entry already matches it.
  nsExec::ExecToLog `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$d = $$env:LIBREPAPER_INSTDIR.TrimEnd('\'); $$k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $$true); if (-not $$k) { $$k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment') }; $$p = $$k.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames); $$e = @(); if ($$p) { $$e = @($$p -split ';' | Where-Object { $$_ -ne '' }) }; if (-not ($$e | Where-Object { $$_.TrimEnd('\') -ieq $$d })) { $$k.SetValue('Path', (($$e + $$d) -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString) }; $$k.Close()"`
  Pop $0
  ; Tell running programs that the environment changed. Registry writes do not broadcast.
  SendMessage 0xFFFF 0x1A 0 "STR:Environment" /TIMEOUT=5000
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  System::Call 'Kernel32::SetEnvironmentVariable(t "LIBREPAPER_INSTDIR", t "$INSTDIR")i'
  ; Drop every entry that matches the install folder. Other entries stay as they are.
  nsExec::ExecToLog `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$d = $$env:LIBREPAPER_INSTDIR.TrimEnd('\'); $$k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $$true); if ($$k) { $$p = $$k.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames); if ($$p) { $$e = @($$p -split ';' | Where-Object { $$_ -ne '' -and $$_.TrimEnd('\') -ine $$d }); $$k.SetValue('Path', ($$e -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString) }; $$k.Close() }"`
  Pop $0
  SendMessage 0xFFFF 0x1A 0 "STR:Environment" /TIMEOUT=5000
!macroend
