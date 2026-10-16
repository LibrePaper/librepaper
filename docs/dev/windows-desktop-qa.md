# Windows desktop app QA

Run this checklist in the Windows VM before a release. Browser tests do not cover the PowerShell installer, SmartScreen or native dialogs.

## Set up the VM

```sh
tools/dev/windows-vm create            # download Windows 11 once into .windows-vm/
tools/dev/windows-vm start             # first boot installs Windows unattended (about 20 minutes)
tools/dev/windows-vm stop              # after the install finishes
tools/dev/windows-vm snapshot clean    # a clean Windows to return to
tools/dev/windows-vm restore clean     # before each run of the checklist
```

- The VM needs about 40 GB free.
- Microsoft often blocks the scripted ISO download. Then `create` prints where to save an ISO you download in a browser.
- `tools/dev/windows-vm reset --yes` reinstalls from the cached ISO.
- `WINDOWS_VM_RAM` and `WINDOWS_VM_CPUS` size the VM.
- The VM opens in a GTK window. Ctrl+Alt+G releases the mouse and keyboard, and closing the window stops the VM.

```sh
tools/dev/windows-vm bootstrap          # run the released PowerShell installer in the VM
tools/dev/windows-vm bootstrap v0.0.39  # or a pinned release
```

- The installer runs as the Quickemu user in a PowerShell window on the VM desktop. The window stays open so you can read its output.

## Test an unreleased build

```sh
make web                               # the app must be built first
tools/dev/windows-vm build             # cross-compiles librepaper.exe into the shared folder
```

```sh
tools/dev/windows-vm bootstrap local   # run install-local.ps1 in the VM
```

## Checklist

1. Run `tools/dev/windows-vm bootstrap`, or run the released installer by hand in PowerShell: `powershell -ExecutionPolicy Bypass -c "irm https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.ps1 | iex"`. Open a new terminal and confirm `librepaper --version`. Record any SmartScreen or Defender warning.
2. Open the desktop app and confirm it loads app.librepaper.org.
3. In Settings > Rendering, use Choose folder under Project folder and confirm a live preview uses that folder. Cancel a second chooser and confirm the prior folder remains.
4. Restore the clean snapshot and repeat step 1 with Scoop: `scoop bucket add vincentarelbundock https://github.com/vincentarelbundock/scoop-bucket` then `scoop install librepaper`.

## Desktop app console flicker regression

- Open the desktop app. It starts the local service in the background. Open Settings > Desktop app. Confirm the page keeps loading and no visible `cmd`, registry or `taskkill` windows appear during 45 to 60 seconds. This covers multiple 5-second state polls and 15-second capabilities polls.
- Repeat once with no discoverable optional tools, then with Quarto, R, Python and Calepin available. Use the explicit rescan action and confirm the connection remains stable without visible console windows in both cases.

Code signing is outside the scope of this checklist.
