import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

// The desktop shell and the local service it starts live in two Cargo
// workspaces, so `make test` never compiles them together. Each names the
// local-service protocol it speaks; when they disagree the service refuses to
// start and every desktop-only page is empty. This pins the two numbers to
// each other, which is what caught nothing on 2026-10-25 when the service
// moved to protocol 3 and the shell still asked for 2.
const repo = fileURLToPath(new URL('../..', import.meta.url));

test('the desktop shell speaks a protocol the local service offers', async () => {
  const service = await readFile(new URL('crates/librepaper-companion/src/local/protocol.rs', `file://${repo}`), 'utf8');
  const shell = await readFile(new URL('desktop/src/service.rs', `file://${repo}`), 'utf8');
  const offered = service.match(/pub const PROTOCOL_VERSIONS: &\[u32\] = &\[([0-9, ]+)\];/);
  const spoken = shell.match(/^const PROTOCOL: u32 = ([0-9]+);/m);
  assert.ok(offered, 'the companion declares PROTOCOL_VERSIONS');
  assert.ok(spoken, 'the desktop shell declares PROTOCOL');
  const versions = offered[1].split(',').map((part) => Number(part.trim())).filter(Number.isFinite);
  assert.ok(versions.includes(Number(spoken[1])), `the shell speaks protocol ${spoken[1]}, the service offers ${versions.join(', ')}`);
});
