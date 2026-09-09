// Boots the Biber guest built by build.mjs. Reads vm.json at /vm/vm.json to
// stay in sync with the actual release (memory size, ready marker) instead
// of hardcoding values that could drift from what build.mjs wrote.
let emulator, output = '', chunk = '', stage = 'booting', active = null, readyMarker = 'LIBREPAPER_VM_READY';

async function start() {
  const vmJson = await (await fetch('/vm/vm.json')).json();
  readyMarker = vmJson.boot?.ready || readyMarker;
  importScripts('/vm/libv86.js');
  emulator = new V86({
    wasm_path: '/vm/v86.wasm',
    memory_size: (vmJson.memory_mb || 256) * 1024 * 1024,
    vga_memory_size: 2 * 1024 * 1024,
    bios: { url: '/vm/seabios.bin' },
    vga_bios: { url: '/vm/vgabios.bin' },
    bzimage: { url: '/vm/bzimage' },
    filesystem: { basefs: '/vm/fs.json', baseurl: '/vm/objects/' },
    cmdline: 'tsc=reliable mitigations=off random.trust_cpu=on',
    autostart: true, disable_keyboard: true, disable_mouse: true,
  });
  setInterval(() => { if (chunk) { postMessage({ type: 'serial', text: chunk }); chunk = ''; } }, 100);
  const send = (s) => emulator.serial0_send(s + '\n');
  emulator.add_listener('serial0-output-byte', (b) => {
    const char = String.fromCharCode(b);
    output = (output + char).slice(-200000);
    chunk += char;
    if (stage === 'booting' && output.endsWith('~% ')) {
      stage = 'mounting';
      // Same boot protocol as latex/benchmark/candidates/tinytex-v86/worker.js:
      // the Buildroot kernel provides the base shell; the packed guest (this
      // recipe's minimal Debian + Biber, not TinyTeX) is mounted at /mnt via 9p.
      send(
        "stty -echo; test -x /mnt/usr/local/bin/biber && mount --bind /dev /mnt/dev && mount -t proc proc /mnt/proc && printf '\\n" +
          readyMarker + "\\n' || printf '\\nLIBREPAPER_VM_FAILED\\n'"
      );
    }
    if (stage === 'mounting' && output.includes('\n' + readyMarker + '\r\n')) {
      stage = 'ready';
      postMessage({ type: 'status', status: 'ready', guestMemoryBytes: (vmJson.memory_mb || 256) * 1024 * 1024 });
    }
    if (stage === 'mounting' && output.includes('\nLIBREPAPER_VM_FAILED\r\n')) {
      stage = 'failed';
      postMessage({ type: 'status', status: 'error', error: 'Guest filesystem setup failed' });
    }
    if (active) {
      const found = output.slice(active.start).match(new RegExp('LIBREPAPER_DONE_' + active.id + ':(\\d+)\\r?\\n'));
      if (found) {
        postMessage({ type: 'command', id: active.id, exitCode: Number(found[1]), milliseconds: performance.now() - active.started });
        active = null;
      }
    }
  });
  onmessage = async ({ data }) => {
    try {
      if (data.type === 'command') {
        if (active) throw new Error('A guest command is already running');
        if (!/^[a-z0-9_]+$/.test(data.id)) throw new Error('Invalid command id');
        active = { id: data.id, start: output.length, started: performance.now() };
        send(data.command + "; printf '\\nLIBREPAPER_DONE_" + data.id + ":%s\\n' \"$?\"");
      } else if (data.type === 'write') {
        await emulator.create_file(data.path, new Uint8Array(data.bytes));
        postMessage({ type: 'file', id: data.id, written: true });
      } else if (data.type === 'read') {
        const bytes = await emulator.read_file(data.path);
        postMessage({ type: 'file', id: data.id, bytes });
      }
    } catch (error) {
      postMessage({ type: data.type === 'command' ? 'command' : 'file', id: data.id, error: String(error) });
    }
  };
}
start().catch((error) => postMessage({ type: 'status', status: 'error', error: String(error) }));
