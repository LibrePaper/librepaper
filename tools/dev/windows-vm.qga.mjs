// Runs the LibrePaper installer inside the Windows VM through the QEMU guest agent.
// Usage: node tools/dev/windows-vm.qga.mjs SOCKET MODE

import { randomInt } from 'node:crypto';
import { once } from 'node:events';
import { connect } from 'node:net';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const GUEST_USER = 'Quickemu';
const GUEST_PASSWORD = 'quickemu';
const GUEST_SCRIPT = 'C:\\Users\\Public\\librepaper-bootstrap.ps1';
const TASK_NAME = 'LibrePaperBootstrap';

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

// Newline-delimited JSON client for one guest agent connection.
class Agent {
  constructor(socket) {
    this.socket = socket;
    this.buffer = '';
    this.messages = [];
    this.waiter = null;
    this.error = null;
    socket.setEncoding('utf8');
    socket.on('data', (chunk) => this.receive(chunk));
    socket.on('error', (err) => this.fail(err));
    socket.on('close', () => this.fail(new Error('connection closed')));
  }

  receive(chunk) {
    this.buffer += chunk;
    let end;
    while ((end = this.buffer.indexOf('\n')) !== -1) {
      const line = this.buffer.slice(0, end).trim();
      this.buffer = this.buffer.slice(end + 1);
      if (line === '') continue;
      try {
        this.deliver(JSON.parse(line));
      } catch {
        this.fail(new Error(`unreadable reply: ${line}`));
      }
    }
  }

  deliver(message) {
    if (this.waiter) {
      const waiter = this.waiter;
      this.waiter = null;
      waiter.resolve(message);
    } else {
      this.messages.push(message);
    }
  }

  fail(err) {
    this.error = this.error ?? err;
    if (this.waiter) {
      const waiter = this.waiter;
      this.waiter = null;
      waiter.reject(this.error);
    }
  }

  // Resolves with the next reply line, or rejects on timeout or a dead socket.
  next(timeoutMs) {
    if (this.messages.length > 0) return Promise.resolve(this.messages.shift());
    if (this.error) return Promise.reject(this.error);
    return new Promise((done, reject) => {
      const timer = setTimeout(() => {
        this.waiter = null;
        reject(new Error(`no reply within ${timeoutMs} ms`));
      }, timeoutMs);
      this.waiter = {
        resolve: (message) => {
          clearTimeout(timer);
          done(message);
        },
        reject: (err) => {
          clearTimeout(timer);
          reject(err);
        },
      };
    });
  }

  write(name, args) {
    this.socket.write(`${JSON.stringify({ execute: name, arguments: args })}\n`);
  }

  // Discards stale replies until the reply to our own id arrives.
  async sync(timeoutMs) {
    const id = randomInt(1, 2 ** 31 - 1);
    this.write('guest-sync', { id });
    for (;;) {
      const reply = await this.next(timeoutMs);
      if (reply.error) throw new Error(`guest-sync: ${reply.error.class}: ${reply.error.desc}`);
      if (reply.return === id) return;
    }
  }

  async call(name, args = {}, timeoutMs = 30000) {
    this.write(name, args);
    let reply;
    try {
      reply = await this.next(timeoutMs);
    } catch (err) {
      throw new Error(`${name}: ${err.message}`);
    }
    if (reply.error) {
      throw new Error(`${name} failed: ${reply.error.class}: ${reply.error.desc}`);
    }
    return reply.return;
  }

  close() {
    this.socket.destroy();
  }
}

async function connectAgent(socketPath) {
  const socket = connect(socketPath);
  const agent = new Agent(socket);
  await once(socket, 'connect');
  return agent;
}

// Retries until the guest agent answers a sync and a ping.
async function waitForAgent(socketPath) {
  const timeoutS = Number(process.env.WINDOWS_VM_AGENT_TIMEOUT ?? 600);
  const retryMs = Number(process.env.WINDOWS_VM_AGENT_RETRY_MS ?? 5000);
  const deadline = Date.now() + timeoutS * 1000;
  let warned = false;
  for (;;) {
    let agent = null;
    try {
      agent = await connectAgent(socketPath);
      await agent.sync(5000);
      await agent.call('guest-ping', {}, 5000);
      return agent;
    } catch {
      agent?.close();
    }
    if (Date.now() >= deadline) {
      throw new Error(`the guest agent did not answer within ${timeoutS} s`);
    }
    if (!warned) {
      console.error('windows-vm: waiting for the guest agent');
      warned = true;
    }
    await sleep(retryMs);
  }
}

function installLine(mode) {
  if (mode === 'latest') {
    return 'irm https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.ps1 | iex';
  }
  if (mode === 'local') {
    return "& '\\\\10.0.2.4\\qemu\\install-local.ps1'";
  }
  return `irm https://github.com/LibrePaper/librepaper/releases/download/${mode}/librepaper-installer.ps1 | iex`;
}

// The PowerShell script that runs inside the guest.
export function guestScript(mode) {
  return [
    "$ErrorActionPreference = 'Stop'",
    installLine(mode),
    "Write-Host 'LibrePaper installed. Open a new terminal and run: librepaper --version'",
  ].join('\r\n') + '\r\n';
}

async function writeGuestFile(agent, path, text) {
  const handle = await agent.call('guest-file-open', { path, mode: 'w' });
  await agent.call('guest-file-write', {
    handle,
    'buf-b64': Buffer.from(text, 'utf8').toString('base64'),
  });
  await agent.call('guest-file-close', { handle });
}

// Runs a program in the guest and waits for it to exit.
async function runGuest(agent, path, args) {
  const { pid } = await agent.call('guest-exec', { path, arg: args, 'capture-output': true });
  for (;;) {
    const status = await agent.call('guest-exec-status', { pid });
    if (status.exited) return status;
    await sleep(500);
  }
}

function printOutput(status) {
  for (const key of ['out-data', 'err-data']) {
    if (status[key]) process.stderr.write(Buffer.from(status[key], 'base64'));
  }
}

async function runSchtasks(agent, args, failure) {
  const status = await runGuest(agent, 'schtasks.exe', args);
  if (status.exitcode !== 0) {
    printOutput(status);
    throw new Error(failure);
  }
}

async function main(argv) {
  const [socketPath, mode] = argv;
  if (!socketPath || !mode) {
    console.error('usage: node tools/dev/windows-vm.qga.mjs SOCKET MODE');
    process.exitCode = 2;
    return;
  }

  const agent = await waitForAgent(socketPath);
  try {
    await writeGuestFile(agent, GUEST_SCRIPT, guestScript(mode));
    const taskRun = `powershell.exe -NoExit -ExecutionPolicy Bypass -File ${GUEST_SCRIPT}`;
    await runSchtasks(
      agent,
      ['/Create', '/F', '/TN', TASK_NAME, '/SC', 'ONCE', '/ST', '00:00',
        '/RU', GUEST_USER, '/RP', GUEST_PASSWORD, '/IT', '/TR', taskRun],
      'schtasks /Create failed',
    );
    await runSchtasks(agent, ['/Run', '/TN', TASK_NAME], 'schtasks /Run failed');
    console.log('windows-vm: the installer is running in a PowerShell window in the VM');
  } finally {
    agent.close();
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((err) => {
    console.error(`windows-vm: ${err.message}`);
    process.exitCode = 1;
  });
}
