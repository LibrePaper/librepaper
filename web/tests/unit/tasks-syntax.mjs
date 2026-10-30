import assert from "node:assert/strict";
import {
  CYCLE, FIELD, isDate, isOverdue, shortDate, todayISO, validateField,
} from "../../src/lib/tasks/syntax.js";

// Field values: each valid value passes, each invalid one returns a warning sentence.
{
  assert.equal(validateField("priority", "1"), null);
  assert.equal(validateField("priority", "5"), null);
  assert.equal(validateField("priority", "0"), "Priority must be 1 to 5.");
  assert.equal(validateField("priority", "6"), "Priority must be 1 to 5.");
  assert.equal(validateField("priority", ""), "priority needs a value.");

  for (const key of ["due", "start", "scheduled", "created", "done", "cancelled"]) {
    assert.equal(validateField(key, "2026-11-05"), null, key);
    assert.notEqual(validateField(key, "2026-02-30"), null, key);
    assert.notEqual(validateField(key, "Nov 5"), null, key);
  }
  assert.equal(validateField("due", "2024-02-29"), null);
  assert.notEqual(validateField("due", "2026-02-29"), null);

  assert.equal(validateField("assignee", "alice,bob"), null);
  assert.equal(validateField("assignee", "alice.b-2_x"), null);
  assert.notEqual(validateField("assignee", "alice,,bob"), null);
  assert.notEqual(validateField("assignee", "alice,"), null);
  assert.notEqual(validateField("assignee", "al ice"), null);

  assert.equal(validateField("estimate", "2h"), null);
  assert.equal(validateField("estimate", "1.5d"), null);
  assert.equal(validateField("estimate", "30m"), null);
  assert.notEqual(validateField("estimate", "2.5.5h"), null);
  assert.notEqual(validateField("estimate", "2x"), null);
  assert.notEqual(validateField("estimate", "h"), null);
}

// Fields take an optional gap of spaces or tabs after the colon. The value is group 3.
{
  const tight = FIELD.exec("due:2026-11-05");
  assert.deepEqual([tight[1], tight[2], tight[3]], ["due", "", "2026-11-05"]);

  const spaced = FIELD.exec("priority:  2 rest");
  assert.deepEqual([spaced[1], spaced[2], spaced[3]], ["priority", "  ", "2"]);

  const tabbed = FIELD.exec("due:\t2026-11-05");
  assert.deepEqual([tabbed[1], tabbed[2], tabbed[3]], ["due", "\t", "2026-11-05"]);

  // Repeat, id and after are no longer fields.
  for (const text of ["repeat:weekly", "id:task-1", "after:task-0"]) {
    FIELD.lastIndex = 0;
    assert.equal(FIELD.exec(text), null, text);
  }
}

// The cycle order: each box character, then the next one, and the last wraps to the first.
assert.deepEqual(CYCLE, [" ", "x", "/", "?", "-"]);

// Calendar dates: real days only, leap years by the Gregorian rule.
{
  assert.equal(isDate("2026-11-05"), true);
  assert.equal(isDate("2026-02-30"), false);
  assert.equal(isDate("2026-02-29"), false);
  assert.equal(isDate("2024-02-29"), true);
  assert.equal(isDate("2000-02-29"), true);
  assert.equal(isDate("1900-02-29"), false);
  assert.equal(isDate("2026-13-01"), false);
  assert.equal(isDate("2026-1-05"), false);
  assert.equal(isDate(""), false);
  assert.equal(isDate(null), false);
}

// Short labels: the year appears only when it is not today's year.
{
  assert.equal(shortDate("2026-11-05", "2026-10-30", "en-US"), "Nov 5");
  assert.equal(shortDate("2027-01-02", "2026-10-30", "en-US"), "Jan 2, 2027");
  assert.equal(shortDate("2026-10-30", "2026-10-30", "en-US"), "Oct 30");
}

// Overdue: incomplete tasks due before today. Today itself is not overdue.
{
  assert.equal(isOverdue("todo", "2026-10-29", "2026-10-30"), true);
  assert.equal(isOverdue("blocked", "2026-10-29", "2026-10-30"), true);
  assert.equal(isOverdue("doing", "2026-10-29", "2026-10-30"), true);
  assert.equal(isOverdue("todo", "2026-10-30", "2026-10-30"), false);
  assert.equal(isOverdue("todo", "2026-10-31", "2026-10-30"), false);
  assert.equal(isOverdue("done", "2026-10-01", "2026-10-30"), false);
  assert.equal(isOverdue("cancelled", "2026-10-01", "2026-10-30"), false);
  assert.equal(isOverdue("todo", null, "2026-10-30"), false);
}

// Today is the local calendar day, read from the local fields of the date.
{
  assert.equal(todayISO(new Date(2026, 0, 5, 12, 0)), "2026-01-05");
  assert.equal(todayISO(new Date(2026, 11, 31, 23, 59)), "2026-12-31");
}
