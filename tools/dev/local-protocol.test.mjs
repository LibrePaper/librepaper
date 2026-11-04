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
  const wire = await readFile(new URL('crates/librepaper-companion/src/local/wire/mod.rs', `file://${repo}`), 'utf8');
  const shellWire = await readFile(new URL('desktop/src/wire.rs', `file://${repo}`), 'utf8');
  const shell = await readFile(new URL('desktop/src/service.rs', `file://${repo}`), 'utf8');
  const major = wire.match(/pub const MAJOR: u32 = ([0-9]+);/);
  const oldest = wire.match(/pub const OLDEST_MAJOR: u32 = ([0-9]+);/);
  const spoken = shellWire.match(/pub const MAJOR: u32 = ([0-9]+);/);
  assert.ok(major && oldest, 'the companion declares MAJOR and OLDEST_MAJOR');
  assert.ok(spoken, 'the desktop shell wire declares MAJOR');
  assert.ok(shell.includes('const PROTOCOL: u32 = crate::wire::MAJOR;'), 'the desktop service speaks the shell wire MAJOR as PROTOCOL');
  const versions = [];
  for (let version = Number(oldest[1]); version <= Number(major[1]); version += 1) versions.push(version);
  assert.ok(versions.includes(Number(spoken[1])), `the shell speaks protocol ${spoken[1]}, the service offers ${versions.join(', ')}`);
});
