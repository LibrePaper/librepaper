import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

const { createLiveChat, MAX_RESIDENT } = await loadRunes(
  new URL("../../src/lib/reader/live-chat.svelte.js", import.meta.url),
);

// UUIDv7-like ids: lowercase hex that sorts by time.
const id = (n) => `0000${n.toString(16).padStart(8, "0")}-0000-7000-8000-000000000000`;
const msg = (n) => ({ type: "chat", id: id(n), text: `m${n}`, creator: "A", created: "2026-01-01T00:00:00Z" });
const ids = (chat) => chat.state.messages.map((m) => m.id);
const range = (a, b) => Array.from({ length: b - a + 1 }, (_, i) => msg(a + i));

// No chat in the hello means no chat access.
{
  const chat = createLiveChat({ slug: "s" });
  chat.seed({ type: "hello", chat: range(1, 3), chat_more: true, chat_can_clear: true });
  assert.equal(chat.state.messages.length, 3);
  assert.equal(chat.state.more, true);
  assert.equal(chat.state.canClear, true);
  chat.seed({ type: "hello" });
  assert.deepEqual(chat.state.messages, []);
  assert.equal(chat.state.more, false);
  assert.equal(chat.state.canClear, false);
}

// A live message is deduplicated and kept in id order.
{
  const chat = createLiveChat({ slug: "s" });
  chat.seed({ chat: range(1, 3), chat_more: false });
  assert.equal(chat.receive(msg(5)), true);
  assert.equal(chat.receive(msg(5)), true);
  assert.equal(chat.receive(msg(4)), true);
  assert.deepEqual(ids(chat), [1, 2, 3, 4, 5].map(id));
  assert.equal(chat.receive({ type: "comment" }), false);
}

// A reconnect whose hello overlaps what is held fills the gap by union.
{
  const chat = createLiveChat({ slug: "s" });
  chat.seed({ chat: range(1, 5), chat_more: true });
  chat.seed({ chat: range(4, 8), chat_more: true, chat_can_clear: false });
  assert.deepEqual(ids(chat), range(1, 8).map((m) => m.id));
  assert.equal(chat.state.more, true);
  const whole = createLiveChat({ slug: "s" });
  whole.seed({ chat: range(3, 5), chat_more: true });
  whole.seed({ chat: range(1, 6), chat_more: false });
  assert.equal(whole.state.more, false, "a hello that reaches further back decides");
  assert.equal(whole.state.messages.length, 6);
}

// A hello that starts after everything held is a gap: it replaces the list.
{
  const chat = createLiveChat({ slug: "s" });
  chat.seed({ chat: range(1, 3), chat_more: false });
  chat.seed({ chat: range(10, 12), chat_more: true });
  assert.deepEqual(ids(chat), range(10, 12).map((m) => m.id));
  assert.equal(chat.state.more, true);
}

// Clearing empties everything.
{
  const chat = createLiveChat({ slug: "s" });
  chat.seed({ chat: range(1, 3), chat_more: true, chat_can_clear: true });
  assert.equal(chat.receive({ type: "chat-cleared" }), true);
  assert.deepEqual(chat.state.messages, []);
  assert.equal(chat.state.more, false);
  assert.equal(chat.state.canClear, true);
}

// At most MAX_RESIDENT messages stay, the oldest dropped.
{
  const chat = createLiveChat({ slug: "s" });
  chat.seed({ chat: range(1, MAX_RESIDENT), chat_more: false });
  assert.equal(chat.state.more, false);
  chat.receive(msg(MAX_RESIDENT + 1));
  assert.equal(chat.state.messages.length, MAX_RESIDENT);
  assert.equal(chat.state.messages[0].id, id(2));
  assert.equal(chat.state.more, true);
}

// Older pages are fetched before the oldest id and prepended.
{
  const calls = [];
  globalThis.fetch = async (url, init) => {
    calls.push({ url, init });
    return { ok: true, status: 200, json: async () => ({ messages: range(1, 3), more: false }) };
  };
  const chat = createLiveChat({ slug: "proj" });
  chat.seed({ chat: range(4, 6), chat_more: true });
  await chat.loadOlder();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, `/api/documents/proj/chat-history?before=${encodeURIComponent(id(4))}&limit=50`);
  assert.deepEqual(ids(chat), range(1, 6).map((m) => m.id));
  assert.equal(chat.state.more, false);
  assert.equal(chat.state.loadingOlder, false);
  await chat.loadOlder();
  assert.equal(calls.length, 1, "nothing is asked when there is nothing older");

  globalThis.fetch = async () => ({ ok: false, status: 500, json: async () => ({ error: "nope" }) });
  const failing = createLiveChat({ slug: "proj" });
  failing.seed({ chat: range(4, 6), chat_more: true });
  await failing.loadOlder();
  assert.equal(failing.state.loadError, "nope");
  assert.equal(failing.state.messages.length, 3);
  assert.equal(failing.state.more, true);
}
