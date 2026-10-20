import assert from "node:assert/strict";
import test from "node:test";
import { chatRows, dayLabel } from "../../src/lib/chat-rows.js";

const options = {
  authorKey: (message) => message.creator || message.role,
  roleLabel: (message) => message.creator || message.role,
};
const now = new Date(2026, 9, 20, 12, 0, 0);

test("same author within five minutes shares a group", () => {
  const rows = chatRows([
    { id: 1, creator: "ana", text: "a", created: new Date(2026, 9, 20, 10, 0).toISOString() },
    { id: 2, creator: "ana", text: "b", created: new Date(2026, 9, 20, 10, 4).toISOString() },
  ], { ...options, now });
  assert.equal(rows[0].starts, true);
  assert.equal(rows[1].starts, false);
  assert.equal(rows[1].date, null);
  assert.notEqual(rows[1].time, "");
});

test("a gap over five minutes starts a new group", () => {
  const rows = chatRows([
    { id: 1, creator: "ana", text: "a", created: new Date(2026, 9, 20, 10, 0).toISOString() },
    { id: 2, creator: "ana", text: "b", created: new Date(2026, 9, 20, 10, 6).toISOString() },
  ], { ...options, now });
  assert.equal(rows[1].starts, true);
});

test("a change of author starts a new group", () => {
  const rows = chatRows([
    { id: 1, creator: "ana", text: "a", created: new Date(2026, 9, 20, 10, 0).toISOString() },
    { id: 2, creator: "ben", text: "b", created: new Date(2026, 9, 20, 10, 1).toISOString() },
  ], { ...options, now });
  assert.equal(rows[1].starts, true);
  assert.equal(rows[1].name, "ben");
});

test("the first message gets a day separator and a day change gets one too", () => {
  const rows = chatRows([
    { id: 1, creator: "ana", text: "a", created: new Date(2026, 9, 19, 23, 50).toISOString() },
    { id: 2, creator: "ana", text: "b", created: new Date(2026, 9, 20, 9, 0).toISOString() },
  ], { ...options, now });
  assert.equal(rows[0].date, "Yesterday");
  assert.equal(rows[0].starts, true);
  assert.equal(rows[1].date, "Today");
  assert.equal(rows[1].starts, true);
  assert.equal(dayLabel(new Date(2026, 9, 20, 1), now), "Today");
  assert.equal(dayLabel(new Date(2026, 9, 19, 1), now), "Yesterday");
});

test("messages without created group by author only, with no date and no time", () => {
  const rows = chatRows([
    { id: 1, role: "user", text: "a" },
    { id: 2, role: "user", text: "b" },
    { id: 3, role: "agent", text: "c" },
  ], { authorKey: (message) => message.role, roleLabel: (message) => message.role, now });
  assert.deepEqual(rows.map((row) => row.starts), [true, false, true]);
  assert.deepEqual(rows.map((row) => row.date), [null, null, null]);
  assert.deepEqual(rows.map((row) => row.time), ["", "", ""]);
  assert.deepEqual(rows.map((row) => row.stamp), ["", "", ""]);
});
