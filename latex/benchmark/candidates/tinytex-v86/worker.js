importScripts('/assets/libv86.js');
const emulator = new V86({
  wasm_path: '/assets/v86.wasm', memory_size: 512 * 1024 * 1024,
  vga_memory_size: 2 * 1024 * 1024,
  bios: { url: '/assets/seabios.bin' }, vga_bios: { url: '/assets/vgabios.bin' },
  bzimage: { url: '/assets/buildroot-bzimage68.bin' },
  filesystem: { basefs: '/assets/fs.json', baseurl: '/assets/objects/' },
  cmdline: 'tsc=reliable mitigations=off random.trust_cpu=on',
  autostart: true, disable_keyboard: true, disable_mouse: true,
});
let output = '', chunk = '', stage = 'booting', active = null;
setInterval(() => { if (chunk) { postMessage({ type: 'serial', text: chunk }); chunk = ''; } }, 100);
const send = s => emulator.serial0_send(s + '\n');
emulator.add_listener('serial0-output-byte', b => {
  const char = String.fromCharCode(b);
  output = (output + char).slice(-200000); chunk += char;
  if (stage === 'booting' && output.endsWith('~% ')) {
    stage = 'mounting';
    // The Buildroot kernel supplies the guest's device/proc mounts. The Debian
    // userland and TinyTeX are served from the lazy 9p filesystem.
    send("stty -echo; test -x /mnt/usr/bin/env && mount --bind /dev /mnt/dev && mount -t proc proc /mnt/proc && printf '\\nLIBREPAPER_VM_READY\\n' || printf '\\nLIBREPAPER_VM_FAILED\\n'");
  }
  if (stage === 'mounting' && output.includes('\nLIBREPAPER_VM_READY\r\n')) {
    stage = 'ready'; postMessage({ type: 'status', status: 'ready', guestMemoryBytes: 512 * 1024 * 1024 });
  }
  if (stage === 'mounting' && output.includes('\nLIBREPAPER_VM_FAILED\r\n')) {
    stage = 'failed'; postMessage({ type: 'status', status: 'error', error: 'Guest filesystem setup failed' });
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
