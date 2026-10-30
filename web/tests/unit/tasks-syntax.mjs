import assert from "node:assert/strict";
import {
  addRepeat, isDate, isOverdue, shortDate, todayISO, validateField,
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

  for (const rule of ["daily", "weekly", "monthly", "yearly", "2w", "1d", "999m", "12y"]) {
    assert.equal(validateField("repeat", rule), null, rule);
  }
  for (const rule of ["fortnightly", "0d", "1000d", "2x", "w"]) {
    assert.notEqual(validateField("repeat", rule), null, rule);
  }

  assert.equal(validateField("id", "task-123"), null);
  assert.notEqual(validateField("id", "task 123"), null);
  assert.equal(validateField("after", "task-122,review"), null);
  assert.notEqual(validateField("after", "task-122,"), null);

  assert.equal(validateField("estimate", "2h"), null);
  assert.equal(validateField("estimate", "1.5d"), null);
  assert.equal(validateField("estimate", "30m"), null);
  assert.notEqual(validateField("estimate", "2.5.5h"), null);
  assert.notEqual(validateField("estimate", "2x"), null);
  assert.notEqual(validateField("estimate", "h"), null);
}

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

// Repeat rules: days and weeks are plain, months and years clamp to the last day.
{
  assert.equal(addRepeat("2026-01-31", "monthly"), "2026-02-28");
  assert.equal(addRepeat("2024-01-31", "1m"), "2024-02-29");
  assert.equal(addRepeat("2026-01-31", "2m"), "2026-03-31");
  assert.equal(addRepeat("2026-11-30", "monthly"), "2026-12-30");
  assert.equal(addRepeat("2026-11-15", "12m"), "2027-11-15");
  assert.equal(addRepeat("2026-10-30", "2w"), "2026-11-13");
  assert.equal(addRepeat("2026-10-30", "3d"), "2026-11-02");
  assert.equal(addRepeat("2026-12-31", "daily"), "2027-01-01");
  assert.equal(addRepeat("2026-10-30", "weekly"), "2026-11-06");
  assert.equal(addRepeat("2024-02-29", "yearly"), "2025-02-28");
  assert.equal(addRepeat("2024-02-29", "1y"), "2025-02-28");
  assert.equal(addRepeat("2028-02-29", "yearly"), "2029-02-28");

  assert.equal(addRepeat("2026-02-30", "weekly"), null);
  assert.equal(addRepeat("2026-01-01", "0d"), null);
  assert.equal(addRepeat("2026-01-01", "fortnightly"), null);
  assert.equal(addRepeat("", "daily"), null);
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
