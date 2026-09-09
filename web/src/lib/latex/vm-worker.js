// The bibliography VM's guest, driven over v86's serial console.
//
// This is a classic (non-module) worker: v86's own `libv86.js` is loaded
// with `importScripts`, which a module worker does not have, so this file
// cannot `import` anything from `vm.js`. The few pure helpers it needs
// (relative-path validation, control-file-mismatch detection) are inlined
// below as literal copies -- each comment says so and points back at the
// canonical, checked copy in `vm.js`, which is what
// `web/checks/latex-vm.mjs` actually exercises.
//
// Protocol with `vm.js` (every message a plain object with `type`):
//
//   in  { type: "boot", config: { libv86Url, wasmPath, biosUrl, vgaBiosUrl,
//                                   bzimageUrl, basefsUrl, baseurl,
//                                   memoryBytes, ready } }
//   out { type: "status", status: "ready" }
//   out { type: "status", status: "error", error }
//
//   in  { type: "job", id, stem, files: [{ path, bytes: ArrayBuffer }] }
//   out { type: "job-result", id, exitCode,
//                              bbl: ArrayBuffer|null, blg: ArrayBuffer|null,
//                              incompatible, biberVersion }
//   out { type: "job-error", id, error }
//
//   in  { type: "cancel", id, graceMs }   // Ctrl-C, then wait up to graceMs
//                                          // for the shell prompt to return
//   out { type: "poisoned" }              // the guest never answered; the
//                                          // job (if any) also gets a
//                                          // job-error, and vm.js retires
//
//   in  { type: "retire" }                // stop the guest; no reply
//
// The guest runs one command at a time, the same discipline
// `tinytex-v86/worker.js` uses: a command is `<cmd>; printf
// '\nLIBREPAPER_DONE_<id>:%s\n' "$?"` and completion is recognised by that
// marker appearing in the serial transcript captured since the command was
// sent. Boot completion is the `~% ` shell prompt, confirmed by one more
// round-trip through the same marker convention so a prompt printed before
// the shell can actually accept input is never mistaken for readiness.

let emulator = null;
let readyMarker = "LIBREPAPER_VM_READY";
let failedMarker = "LIBREPAPER_VM_FAILED";
let setupCommand = null; // the descriptor's one-time guest setup, run at the first prompt
let execPrefix = ""; // how a job command enters the guest (a chroot, usually)
let output = "";
let stage = "booting"; // "booting" | "confirming" | "ready"
let active = null; // { id, stem } -- the job currently staging/running/reading
let pendingCommand = null; // { markerId, start, resolve, reject } -- one guest command
let canceling = null; // { graceTimer, promptStart }

function send(command) {
  emulator.serial0_send(command + "\n");
}

function shellQuote(path) {
  return "'" + String(path).replaceAll("'", "'\\''") + "'";
}

// Inlined copy of `validateRelativePath` from vm.js -- keep the two in sync.
function validateRelativePath(path) {
  if (
    typeof path !== "string" || !path || path.startsWith("/") || path.includes("\\") ||
    path.split("/").some((part) => !part || part === "." || part === "..") ||
    /[\u0000-\u001f\u007f]/.test(path)
  ) {
    throw new TypeError(`Invalid relative guest path: ${String(path)}`);
  }
  return path;
}

// Inlined copy of `detectIncompatible` from vm.js.
function detectIncompatible(blg) {
  return /control file version/i.test(String(blg || ""));
}

onmessage = ({ data }) => {
  try {
    if (data.type === "boot") { boot(data.config); return; }
    if (data.type === "job") { runJob(data); return; }
    if (data.type === "cancel") { cancelActive(data); return; }
    if (data.type === "retire") { retireGuest(); return; }
  } catch (error) {
    postMessage({ type: "job-error", id: data?.id, error: String(error?.message || error) });
  }
};

function boot(config) {
  readyMarker = config.ready || readyMarker;
  failedMarker = config.failed || failedMarker;
  setupCommand = config.setup || null;
  execPrefix = config.exec ? `${config.exec} ` : "";
  importScripts(config.libv86Url);
  emulator = new V86({
    wasm_path: config.wasmPath,
    memory_size: config.memoryBytes,
    vga_memory_size: 2 * 1024 * 1024,
    ...(config.cmdline ? { cmdline: config.cmdline } : {}),
    bios: { url: config.biosUrl },
    vga_bios: { url: config.vgaBiosUrl },
    bzimage: { url: config.bzimageUrl },
    filesystem: { basefs: config.basefsUrl, baseurl: config.baseurl },
    autostart: true,
    disable_keyboard: true,
    disable_mouse: true,
  });
  emulator.add_listener("serial0-output-byte", onSerialByte);
}

function onSerialByte(byte) {
  const char = String.fromCharCode(byte);
  output = (output + char).slice(-200000);

  if (stage === "booting" && output.endsWith("~% ")) {
    stage = "confirming";
    // The descriptor's setup mounts the guest and prints the ready marker
    // itself; without one, the marker is printed directly, which confirms
    // the shell reads commands rather than trusting the prompt alone.
    send(setupCommand || `printf '\\n${readyMarker}\\n'`);
    return;
  }
  if (stage === "confirming" && output.includes(`\n${readyMarker}\r\n`)) {
    stage = "ready";
    postMessage({ type: "status", status: "ready" });
    return;
  }
  if (stage === "confirming" && output.includes(`\n${failedMarker}\r\n`)) {
    stage = "failed";
    postMessage({ type: "status", status: "error", error: "Guest filesystem setup failed" });
    return;
  }

  if (canceling) {
    if (output.slice(canceling.promptStart).endsWith("~% ")) {
      clearTimeout(canceling.graceTimer);
      canceling = null;
      if (pendingCommand) {
        const command = pendingCommand;
        pendingCommand = null;
        const error = new Error("Canceled");
        error.name = "Canceled";
        command.reject(error);
      }
    }
    return;
  }

  if (pendingCommand) {
    const found = output.slice(pendingCommand.start).match(new RegExp("LIBREPAPER_DONE_" + pendingCommand.markerId + ":(\\d+)\\r?\\n"));
    if (found) {
      const command = pendingCommand;
      pendingCommand = null;
      command.resolve(Number(found[1]));
    }
  }
}

function runCommand(command, markerId) {
  return new Promise((resolve, reject) => {
    if (pendingCommand) { reject(new Error("A guest command is already running")); return; }
    if (!/^[A-Za-z0-9_]+$/.test(markerId)) { reject(new Error("Invalid command id")); return; }
    pendingCommand = { markerId, start: output.length, resolve, reject };
    // The command runs inside the guest; the marker is printed by the outer
    // shell afterwards, with the guest command's exit status.
    const inside = execPrefix ? `${execPrefix}${shellQuote(command)}` : command;
    send(`${inside}; printf '\\nLIBREPAPER_DONE_${markerId}:%s\\n' "$?"`);
  });
}

async function safeRead(path) {
  try {
    const bytes = await emulator.read_file(path);
    if (bytes == null) return null;
    return bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  } catch {
    return null;
  }
}

async function runJob(data) {
  if (active) throw new Error("A guest job is already running");
  const id = data.id;
  const stem = data.stem;
  const files = data.files.map((f) => ({ path: validateRelativePath(f.path), bytes: new Uint8Array(f.bytes) }));
  const dir = `/work/${id}`;
  active = { id, stem };

  try {
    const dirs = new Set([dir]);
    for (const file of files) {
      const parts = file.path.split("/");
      for (let i = 1; i < parts.length; i++) dirs.add(`${dir}/${parts.slice(0, i).join("/")}`);
    }
    const mkdirExit = await runCommand("mkdir -p " + [...dirs].map(shellQuote).join(" "), `${id}_mkdir`);
    if (mkdirExit !== 0) throw new Error(`Cannot create guest workspace (exit ${mkdirExit})`);

    for (const file of files) await emulator.create_file(`${dir}/${file.path}`, file.bytes);

    const biberExit = await runCommand(
      `cd ${shellQuote(dir)} && biber --output-format=bbl ${shellQuote(`${stem}.bcf`)}`,
      `${id}_biber`,
    );

    const bbl = await safeRead(`${dir}/${stem}.bbl`);
    const blg = await safeRead(`${dir}/${stem}.blg`);

    // Best effort: a failed cleanup does not invalidate a produced result,
    // and the workspace is scoped to this job id so a stray leftover cannot
    // collide with the next one.
    await runCommand(`rm -rf ${shellQuote(dir)}`, `${id}_cleanup`).catch(() => {});

    active = null;
    postMessage(
      {
        type: "job-result",
        id,
        exitCode: biberExit,
        bbl: bbl ? bbl.buffer : null,
        blg: blg ? blg.buffer : null,
        incompatible: detectIncompatible(blg ? new TextDecoder().decode(blg) : ""),
        biberVersion: null,
      },
      [bbl?.buffer, blg?.buffer].filter(Boolean),
    );
  } catch (error) {
    active = null;
    pendingCommand = null;
    postMessage({ type: "job-error", id, error: String(error?.message || error) });
  }
}

function cancelActive(data) {
  if (!active || active.id !== data.id || canceling) return;
  emulator.serial0_send("\x03");
  const graceMs = data.graceMs || 5000;
  canceling = {
    promptStart: output.length,
    graceTimer: setTimeout(() => {
      // The guest never answered. It is not safe to trust its state again;
      // vm.js treats this message as a reason to retire and re-`prepare()`.
      canceling = null;
      active = null;
      pendingCommand = null;
      postMessage({ type: "poisoned" });
    }, graceMs),
  };
}

function retireGuest() {
  try { emulator?.stop?.(); } catch { /* best effort */ }
  try { emulator?.destroy?.(); } catch { /* best effort */ }
  emulator = null;
  stage = "booting";
  output = "";
  active = null;
  pendingCommand = null;
  if (canceling) { clearTimeout(canceling.graceTimer); canceling = null; }
}
