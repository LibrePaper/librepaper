// Browser regression check for the History panel in
// web/src/components/reader/History.svelte, the list of the document's events.
//
// The panel lists events, newest first, and not raw versions. A stretch of
// autosaves that follow each other within half an hour is one editing event,
// and its versions stay folded until its caret is opened. A push is always its
// own event, titled by who pushed it. Two or more people editing
// in one stretch make it a collaborative event. A named version is always its
// own saved event, and it splits the editing around it. Days from the last
// week each get a heading, and older days fold into the rest of their month,
// into months and into years. The heading that holds the viewed version opens
// by itself, and the live document heads the list as the current version.
//
// The panel is controlled from this check through a small entry that mounts it
// and changes its props at runtime, and that records what it asks for: views,
// comparisons, names, older pages, saved versions and retries.
//
// The check runs in a real browser with the panel's own stylesheet, so the
// layout checks measure real boxes. The browser is launched in UTC so the day
// headings match the dates written below. Every fixture is dated from the real
// current date, so the "Today" and "Yesterday" headings are deterministic.
import assert from "node:assert/strict";
import { build } from "vite";
import tailwindcss from "@tailwindcss/vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, writeFileSync, existsSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { assertFreshDist } from "../helpers/fresh-dist.mjs";

process.env.TZ = "UTC";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-history-panel-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "browser");
const entry = join(temporary, "entry.js");

/* ------------------------------------------------------------- fixtures */

// Every date is taken from the real current date in UTC, so that today,
// yesterday and the older days land where the panel's headings will say.
const now = new Date();
const YEAR = now.getUTCFullYear();
const MONTH = now.getUTCMonth();
const DAY = now.getUTCDate();
const ymd = (value) => value.toISOString().slice(0, 10);
const TODAY = ymd(new Date(Date.UTC(YEAR, MONTH, DAY)));

// The time at `hour:minute` UTC, the given number of days before today.
const at = (daysAgo, hour, minute = 0) =>
  new Date(Date.UTC(YEAR, MONTH, DAY - daysAgo, hour, minute)).toISOString();
// The middle of the month the given number of months before this one.
const monthsAgo = (months, hour) =>
  new Date(Date.UTC(YEAR, MONTH - months, 15, hour)).toISOString();

// One point as the panel reads it. The sha is 64 hex digits: a tag that tells
// the fixtures apart, then the number of the point.
const point = (tag, number, when, by, why, label = "") => ({
  sha: tag + String(number).padStart(63, "0"),
  at: when,
  by,
  why,
  label,
});
// Points in chronological order, each with the ascending seq the panel sorts by.
const ordered = (list) =>
  [...list]
    .sort((left, right) => Date.parse(left.at) - Date.parse(right.at))
    .map((one, index) => ({ ...one, seq: index + 1 }));
const shaAt = (list, when) => list.find((one) => one.at === when).sha;

// A hundred autosaves in one morning, two minutes apart.
const autosaves = ordered(
  Array.from({ length: 100 }, (_, index) => point("1", index, at(0, 8, 2 * index), "Vincent", "sync")),
);

// Four autosaves, a named version, then four more: the named version ends the
// session, so the editing around it is two events.
const named = ordered([
  ...[0, 2, 4, 6].map((minute) => point("2", minute, at(0, 8, minute), "Vincent", "sync")),
  point("2", 8, at(0, 8, 8), "Vincent", "cli", "Before the rewrite"),
  ...[10, 12, 14, 16].map((minute) => point("2", minute, at(0, 8, minute), "Vincent", "sync")),
]);

// Three pushes a minute apart: three events, each titled by its author and
// carrying its message.
const pushes = ordered([
  point("3", 0, at(0, 9, 0), "Alice", "push", "Fix the abstract"),
  point("3", 1, at(0, 9, 1), "Alice", "push", "Add figure 2"),
  point("3", 2, at(0, 9, 2), "Bob", "push", "Tidy the bibliography"),
]);

// Two people in one stretch of editing: a collaborative event.
const collab = ordered([
  point("5", 0, at(0, 9, 0), "Vincent", "sync"),
  point("5", 1, at(0, 9, 2), "Anne", "sync"),
  point("5", 2, at(0, 9, 4), "Vincent", "sync"),
]);

// One of each period: today, yesterday, a day ten days back, a month two
// months back and a year fourteen months back. The day ten days back folds
// into the rest of this month, or into a month group when it falls in an
// earlier month of this year, so the check finds its heading by its key.
const timeline = ordered([
  point("6", 0, at(0, 9, 0), "Vincent", "sync"),
  point("6", 1, at(0, 9, 3), "Vincent", "sync"),
  point("6", 2, at(1, 9, 0), "Vincent", "sync"),
  point("6", 3, at(1, 9, 2), "Vincent", "sync"),
  point("6", 4, at(1, 9, 4), "Vincent", "sync"),
  point("6", 5, at(10, 9, 0), "Vincent", "sync"),
  point("6", 6, at(10, 10, 0), "Vincent", "sync"),
  point("6", 7, monthsAgo(2, 9), "Vincent", "sync"),
  point("6", 8, monthsAgo(14, 9), "Vincent", "sync"),
]);

const olderDay = ymd(new Date(Date.UTC(YEAR, MONTH, DAY - 10)));
const olderKey = olderDay.slice(0, 7) === TODAY.slice(0, 7)
  ? `rest:${olderDay.slice(0, 7)}`
  : olderDay.slice(0, 4) === TODAY.slice(0, 4)
    ? `month:${olderDay.slice(0, 7)}`
    : `year:${olderDay.slice(0, 4)}`;

// Named versions, newest first, as the saved view lists them.
const saved = [
  point("7", 2, at(0, 10, 0), "Vincent", "cli", "Submitted draft"),
  point("7", 1, at(3, 15, 30), "Vincent", "cli", "Before the rewrite"),
];

// A name of exactly 200 characters, for the narrow layout check.
const longName = "A saved version with a name long enough to run past the edge of a narrow panel. "
  .repeat(3)
  .slice(0, 200);
const longSaved = [point("8", 1, at(2, 9, 0), "Vincent", "cli", longName)];

const fixtures = {
  autosaves,
  named,
  pushes,
  collab,
  timeline,
  saved,
  longSaved,
  authorRows: [
    { key: "account:a", name: "Ada", agent: false, unknown: false, colour: "#0072b2", chars: 75, share: 0.75, last: "2026-11-02T09:00:00Z" },
    { key: "unknown", name: "Unknown author", agent: false, unknown: true, colour: "", chars: 25, share: 0.25, last: null },
  ],
  longName,
  keys: {
    today: `day:${TODAY}`,
    older: olderKey,
    oldest: `year:${monthsAgo(14, 9).slice(0, 4)}`,
    olderSha: shaAt(timeline, at(10, 9, 0)),
  },
};

/* ------------------------------------------------------------- the checks */

// Runs in the page. It returns "passed", or the number and name of the check
// that failed and why, so the Node side can report it.
async function runChecks() {
  const check = (condition, message) => {
    if (!condition) throw new Error(message);
  };
  const all = (selector, root = document) => [...root.querySelectorAll(selector)];
  const q = (selector, root = document) => root.querySelector(selector);
  const text = (node) => (node ? node.textContent.replace(/\s+/g, " ").trim() : "");
  const headingOf = (key) => q(`[data-period="${key}"] > button.hx-heading`);
  const eventsOf = (type) => all(`li.hx-event[data-type="${type}"]`);
  const versionsOf = (node) => all("li.hx-version", node);
  const statusText = () => all("li.hx-status").map(text).join(" | ");
  // The views are the shared tab strip's tabs, named by their visible label.
  const choose = (label) => all('[role="tab"]').find((node) => text(node) === label);
  const selected = (label) => choose(label)?.getAttribute("aria-selected") === "true";
  const click = (node, init = {}) =>
    node.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, ...init }));
  const press = (node, name) => {
    node.focus();
    node.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));
  };
  const { keys } = fixtures;

  let current = "setup";
  const step = async (name, body) => {
    current = name;
    await body();
  };

  try {
    await step("1: empty and loading history", async () => {
      mount({ labels: [], loaded: true });
      await flush();
      check(statusText().includes("No previous versions"),
        `a loaded, empty history should say so, shows: ${statusText()}`);
      check(all("li.hx-current").length === 1, "the current version row is listed");

      mount({ labels: [], loaded: false });
      await flush();
      check(statusText().includes("Loading history…"),
        `a history not loaded yet should say so, shows: ${statusText()}`);
      check(all("li.hx-current").length === 1, "the current version row is listed while loading");
    });

    await step("2: autosaves fold into one event", async () => {
      mount({ labels: fixtures.autosaves });
      await flush();
      const runs = () => eventsOf("editing");
      check(runs().length === 1, `a hundred autosaves are one editing event, found ${runs().length}`);
      const run = () => runs()[0];
      // The session holding the live document starts open, so its Current
      // row is in view; closing it hides the versions and opening shows five.
      const caret = () => run().querySelector("button[aria-expanded]");
      check(caret() && caret().getAttribute("aria-expanded") === "true", "the newest session starts open");
      click(caret());
      await flush();
      check(versionsOf(run()).length === 0, "closing the session hides its version rows");

      click(caret());
      await flush();
      check(caret().getAttribute("aria-expanded") === "true" && versionsOf(run()).length === 5,
        `opening the caret shows five versions, shows ${versionsOf(run()).length}`);
      const more = () => run().querySelector(".hx-more");
      check(more() && text(more()) === "Show 50 more versions",
        `the rest are offered as "Show 50 more versions", offered: ${more() ? text(more()) : "nothing"}`);

      click(more());
      await flush();
      check(versionsOf(run()).length === 55,
        `showing more brings the list to 55 versions, shows ${versionsOf(run()).length}`);
    });

    await step("3: a session row only opens and closes, the newest version is the live document, shift-click compares", async () => {
      mount({ labels: fixtures.autosaves });
      await flush();
      const oldest = fixtures.autosaves[0].sha;
      const second = fixtures.autosaves[fixtures.autosaves.length - 2].sha;
      const session = () => eventsOf("editing")[0].querySelector("button.hx-session");
      const rowOf = (sha) => q(`li.hx-version[data-sha="${sha}"] button`);

      // The newest session starts open, since it holds the live document.
      click(session());
      await flush();
      check(calls.length === 0, `closing a session selects nothing, calls: ${JSON.stringify(calls)}`);
      check(session().getAttribute("aria-expanded") === "false", "a click on the open session row closes it");
      click(session());
      await flush();
      check(session().getAttribute("aria-expanded") === "true" && calls.length === 0,
        "a second click opens it, still selecting nothing");
      press(session(), "ArrowLeft");
      await flush();
      check(session().getAttribute("aria-expanded") === "false", "ArrowLeft closes a session");
      press(session(), "ArrowRight");
      await flush();
      check(session().getAttribute("aria-expanded") === "true", "ArrowRight opens a session");

      const first = versionsOf(eventsOf("editing")[0])[0];
      check(first.querySelector(".hx-now") && text(first.querySelector(".hx-now")) === "Current",
        "the newest version row says Current");
      check(all(".hx-now").length === 1, "only the newest version says Current");
      click(first.querySelector("button"));
      await flush();
      check(JSON.stringify(calls) === JSON.stringify([["view", ""]]),
        `the newest row views the live document, calls: ${JSON.stringify(calls)}`);

      // Nothing is viewed yet, so shift-click compares from the live document.
      click(rowOf(second), { shiftKey: true });
      await flush();
      check(JSON.stringify(calls.slice(1)) === JSON.stringify([["compare", "", second]]),
        `a shift-click with nothing viewed should compare from the current version, calls: ${JSON.stringify(calls)}`);

      update({ viewing: oldest });
      await flush();
      click(rowOf(second), { shiftKey: true });
      await flush();
      check(JSON.stringify(calls.slice(2)) === JSON.stringify([["compare", oldest, second]]),
        `a shift-click should compare the viewed version with the clicked one, calls: ${JSON.stringify(calls)}`);
    });

    await step("4: a named version is its own event and splits the session", async () => {
      mount({ labels: fixtures.named });
      await flush();
      const editing = eventsOf("editing");
      check(editing.length === 2, `a named version inside the run leaves two editing events, found ${editing.length}`);
      const saves = eventsOf("saved");
      check(saves.length === 1 && text(saves[0]).includes("Before the rewrite"),
        `the named version is one saved event showing its name, found: ${saves.map(text).join(" | ")}`);
    });

    await step("5: each push is its own event, titled by who pushed it", async () => {
      mount({ labels: fixtures.pushes });
      await flush();
      const found = eventsOf("push");
      check(found.length === 3, `three pushes a minute apart are three events, found ${found.length}`);
      check(text(found[0]).includes("Pushed by Bob") && text(found[0]).includes("Tidy the bibliography"),
        `the newest push names Bob and its message, found: ${text(found[0])}`);
      check(text(found[2]).includes("Pushed by Alice") && text(found[2]).includes("Fix the abstract"),
        `the oldest push names Alice and its message, found: ${text(found[2])}`);
    });

    await step("6: two authors make a collaborative session", async () => {
      mount({ labels: fixtures.collab });
      await flush();
      const shared = eventsOf("collaborative");
      check(shared.length === 1 && text(shared[0]).includes("2 contributors"),
        `two authors in one session are a collaborative event with 2 contributors, found ${shared.map(text).join(" | ")}`);
      check(!text(shared[0]).includes("Collaborative session") && /\d:\d\d/.test(text(shared[0])),
        `a session is titled by its time range, found: ${text(shared[0])}`);
    });

    await step("7: period headings, counts and loading older history", async () => {
      mount({ labels: fixtures.timeline });
      await flush();
      const today = headingOf(keys.today);
      check(all('[data-period^="day:"] > button[aria-expanded="true"]').includes(today),
        "today's heading is open");

      const older = headingOf(keys.older);
      check(older, `the older period ${keys.older} has a heading`);
      check(older.getAttribute("aria-expanded") === "false", `the older period ${keys.older} starts closed`);
      check(text(q(".hx-count", older)) === "2 versions",
        `the older heading counts its versions, says: ${text(q(".hx-count", older))}`);

      mount({ labels: fixtures.timeline, complete: false });
      await flush();
      const oldest = headingOf(keys.oldest);
      check(oldest && text(q(".hx-count", oldest)).includes("+"),
        `the oldest heading of a partial history has a plus, says: ${oldest ? text(q(".hx-count", oldest)) : "no heading"}`);
      const olderButton = () => all("li.hx-older button").find((node) => text(node) === "Load older history");
      check(q("li.hx-older") && olderButton(), "a Load older history button is listed at the end");
      const visibleBefore = all("li.hx-event").length;
      check(visibleBefore > 0, "some events are on the screen");

      click(olderButton());
      await flush();
      check(calls.some((call) => call[0] === "loadmore"),
        `Load older history asks for an older page, calls: ${JSON.stringify(calls)}`);

      mount({ labels: fixtures.timeline, complete: false, moreProblem: "Older history could not be loaded." });
      await flush();
      check(text(q("li.hx-older")).includes("Older history could not be loaded."),
        `a failed older page shows its message: ${text(q("li.hx-older"))}`);
      const retry = all("li.hx-older button").find((node) => text(node) === "Retry");
      check(retry, "a failed older page offers a Retry button");
      check(all("li.hx-event").length === visibleBefore,
        `the loaded rows stay when an older page fails: ${all("li.hx-event").length} of ${visibleBefore}`);
      click(retry);
      await flush();
      check(calls.some((call) => call[0] === "loadmore"),
        `Retry asks for the older page again, calls: ${JSON.stringify(calls)}`);
    });

    await step("8: a viewed version opens its closed period", async () => {
      mount({ labels: fixtures.timeline });
      await flush();
      check(headingOf(keys.older).getAttribute("aria-expanded") === "false", "the older period starts closed");

      update({ viewing: keys.olderSha });
      await flush();
      check(headingOf(keys.older).getAttribute("aria-expanded") === "true",
        "viewing a version inside a closed period opens that period");
      const marked = () => q(`[data-sha="${keys.olderSha}"] [aria-current="true"]`);
      check(marked(), "the version holding the viewed sha is marked aria-current");

      click(headingOf(keys.today));
      await flush();
      check(headingOf(keys.today).getAttribute("aria-expanded") === "false", "the unrelated period collapses");
      check(marked(), "the marked version keeps aria-current after an unrelated period collapses");
    });

    await step("9: saved versions view", async () => {
      mount({ labels: fixtures.timeline, saved: [], savedLoaded: true });
      await flush();
      click(choose("Saved"));
      await flush();
      check(selected("Saved") && !selected("All"),
        "the saved tab is selected and the all tab is not");
      check(calls.some((call) => call[0] === "loadsaved"),
        `choosing saved versions asks for them, calls: ${JSON.stringify(calls)}`);
      check(statusText().includes("No saved versions yet."),
        `an empty saved list says so, shows: ${statusText()}`);

      update({ saved: fixtures.saved });
      await flush();
      const rows = all("li.hx-saved");
      const names = rows.map((row) => text(q(".hx-line1", row)));
      check(names.join("|") === "Submitted draft|Before the rewrite",
        `the saved rows show their names, shows: ${names.join("|")}`);

      update({ viewing: keys.olderSha });
      await flush();
      click(choose("All"));
      await flush();
      check(selected("All") && !selected("Saved"),
        "switching back selects the all tab");
      check(q(".hx-now") && q(`[data-sha="${keys.olderSha}"] [aria-current="true"]`),
        "switching back restores the timeline and the selection in it");
    });

    await step("10: no horizontal overflow at 240px, long names truncate", async () => {
      document.body.style.width = "240px";
      mount({ labels: fixtures.timeline });
      await flush();
      const list = () => q("ol.hx-list");
      check(list().scrollWidth <= list().clientWidth + 1,
        `the timeline overflows sideways at 240px: ${list().scrollWidth} wide in ${list().clientWidth}`);

      mount({ labels: fixtures.timeline, saved: fixtures.longSaved, savedLoaded: true });
      await flush();
      click(choose("Saved"));
      await flush();
      check(list().scrollWidth <= list().clientWidth + 1,
        `the saved list overflows sideways at 240px: ${list().scrollWidth} wide in ${list().clientWidth}`);
      const line = q("li.hx-saved .hx-line1");
      check(line, "the long saved name is listed");
      check(line.scrollWidth > line.clientWidth,
        `the long name is truncated in its line: ${line.scrollWidth} wide in ${line.clientWidth}`);
      const main = q("li.hx-saved button.hx-main");
      check(main && main.title.includes(fixtures.longName),
        "the full name is in the button title");
      document.body.style.width = "";
    });

    await step("11: keyboard moves between rows and opens a closed period", async () => {
      mount({ labels: fixtures.timeline });
      await flush();
      const todayHeading = headingOf(keys.today);
      const navs = all("ol.hx-list [data-nav]");
      const next = navs[navs.indexOf(todayHeading) + 1];
      check(next, "there is a row after the today heading to move to");
      press(todayHeading, "ArrowDown");
      check(document.activeElement === next,
        `ArrowDown on a period heading moves focus to the next row, focus is on: ${document.activeElement?.className}`);

      const older = headingOf(keys.older);
      check(older.getAttribute("aria-expanded") === "false", "the older period starts closed");
      press(older, "ArrowRight");
      await flush();
      check(headingOf(keys.older).getAttribute("aria-expanded") === "true",
        "ArrowRight on a closed period opens it");
    });

    await step("12: no restore or overflow controls, and Compare with… only on the selected row", async () => {
      mount({ labels: fixtures.autosaves, canEdit: true });
      await flush();
      { const opener = eventsOf("editing")[0].querySelector("button[aria-expanded]"); if (opener.getAttribute("aria-expanded") !== "true") click(opener); }
      await flush();
      const panel = q(".timeline");
      check(versionsOf(panel).length === 5, "versions are on the screen to check");
      check(!all(".btn", panel).some((node) => text(node) === "Restore"),
        "no Restore button is in the panel");
      const moreIcon = all("svg", panel).some((svg) =>
        all("circle", svg).filter((circle) => Number(circle.getAttribute("r")) === 1).length === 3);
      check(!moreIcon, "no more-horizontal icon is in the panel");
      const compareButtons = () => all("button[data-compare]", panel);
      check(compareButtons().length === 0, "no row offers Compare with… when nothing is selected");
      check(!q("li.hx-current"), "no separate Current row is listed once there are versions");

      const second = fixtures.autosaves[fixtures.autosaves.length - 2].sha;
      update({ viewing: second });
      await flush();
      check(compareButtons().length === 1 && text(compareButtons()[0]) === "Compare with…",
        `only the selected row offers Compare with…, found ${compareButtons().length}`);
      check(compareButtons()[0].closest("[data-sha]")?.dataset.sha === second,
        "the Compare with… button sits in the selected row");
    });

    await step("13: Compare with… picks the second version, Escape cancels", async () => {
      mount({ labels: fixtures.timeline, viewing: keys.olderSha });
      await flush();
      const compare = () => q("button[data-compare]");
      const status = () => q(".hx-pick");
      check(compare() && text(compare()) === "Compare with…", "the selected row offers Compare with…");

      // A plain click on a version of today's session views it, and a pick on
      // it compares with it. The session row only opens.
      { const opener = eventsOf("editing")[0].querySelector("button.hx-session"); if (opener.getAttribute("aria-expanded") !== "true") click(opener); }
      await flush();
      check(calls.length === 0, "an open session has selected nothing");
      const todayRow = () => versionsOf(eventsOf("editing")[0])[1].querySelector("button");
      const todayNewest = versionsOf(eventsOf("editing")[0])[1].dataset.sha;
      click(todayRow());
      await flush();
      check(JSON.stringify(calls.splice(0)) === JSON.stringify([["view", todayNewest]]),
        "a plain click on a version of today's session views it");

      click(compare());
      await flush();
      check(status() && text(status()).includes("Choose a version to compare with."),
        `choosing Compare with… asks for a version, shows: ${status() ? text(status()) : "nothing"}`);
      check(!compare(), "the Compare with… button is hidden while picking");

      click(todayRow());
      await flush();
      check(JSON.stringify(calls) === JSON.stringify([["compare", keys.olderSha, todayNewest]]),
        `a row while picking should compare the two versions, calls: ${JSON.stringify(calls)}`);
      check(!status(), "pick mode ends after a row is chosen");
      check(compare(), "the Compare with… button is back once pick mode ends");

      click(compare());
      await flush();
      q(".timeline").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
      await flush();
      check(!status(), "Escape ends pick mode");
      check(calls.length === 1, `Escape should not compare, calls: ${JSON.stringify(calls)}`);
      check(document.activeElement === compare(), "focus returns to Compare with… after Escape");
    });

    await step("14: the newest row stands for the live document, endpoints are words", async () => {
      const newest = fixtures.autosaves[fixtures.autosaves.length - 1].sha;
      const second = fixtures.autosaves[fixtures.autosaves.length - 2].sha;
      mount({ labels: fixtures.autosaves, older: second, newer: "current", viewing: second });
      await flush();
      { const opener = eventsOf("editing")[0].querySelector("button.hx-session"); if (opener.getAttribute("aria-expanded") !== "true") click(opener); }
      await flush();
      const live = q(`li.hx-version[data-sha="${newest}"] button`);
      check(live.getAttribute("aria-label").includes("Current") && live.getAttribute("aria-label").includes("Newer"),
        `the live row names Current and the Newer end, says: ${live.getAttribute("aria-label")}`);
      check(!text(live).includes("Newer"), "Current alone is enough on the newer end, the row shows no Newer");
      const old = q(`li.hx-version[data-sha="${second}"] button`);
      check(old.getAttribute("aria-label").includes("Older") && text(old).includes("Older"),
        `the older end says Older, says: ${old.getAttribute("aria-label")}`);
      check(old.getAttribute("aria-current") === "true" && live.getAttribute("aria-current") !== "true",
        "only the viewed version is selected");
      check(q("li.hx-current") === null && !all(".hx-tag").length, "no Current row and no pills");
    });

    await step("13: the authors tab lists who wrote the file and isolates one", async () => {
      mount({ labels: fixtures.timeline });
      await flush();
      check(!calls.some((call) => call[0] === "authorsview"), "the authors view is off until its tab is chosen");
      click(choose("Authors"));
      await flush();
      check(selected("Authors"), "the authors tab is selected");
      check(JSON.stringify(calls.filter((call) => call[0] === "authorsview")) === JSON.stringify([["authorsview", true]]),
        `choosing the tab turns the view on, calls: ${JSON.stringify(calls)}`);
      update({ authors: { rows: fixtures.authorRows, since: "2026-11-01T10:00:00Z", loading: false, error: "", noFile: false } });
      await flush();
      const rows = all("button.hx-author");
      check(rows.map((row) => text(q(".hx-line1", row))).join("|") === "Ada|Unknown author",
        `the rows name each author, shows: ${rows.map((row) => text(row)).join("|")}`);
      check(text(rows[0]).includes("75%"), `a row gives its share, shows: ${text(rows[0])}`);
      click(rows[0]);
      await flush();
      check(JSON.stringify(calls.at(-1)) === JSON.stringify(["isolate", "account:a"]),
        `clicking a row isolates it, calls: ${JSON.stringify(calls)}`);
      update({ authorsOnly: "account:a" });
      await flush();
      check(all("button.hx-author")[0].getAttribute("aria-pressed") === "true", "the isolated row is pressed");
      click(all("button.hx-author")[0]);
      await flush();
      check(JSON.stringify(calls.at(-1)) === JSON.stringify(["isolate", null]),
        `clicking it again shows everyone, calls: ${JSON.stringify(calls)}`);
      click(choose("All"));
      await flush();
      check(JSON.stringify(calls.at(-1)) === JSON.stringify(["authorsview", false]),
        `leaving the tab turns the view off, calls: ${JSON.stringify(calls)}`);
    });

    return "passed";
  } catch (error) {
    return `failed at ${current}: ${error.message}`;
  } finally {
    unmount();
    document.body.style.width = "";
  }
}

/* ----------------------------------------------------------- the harness */

const source = `
import History from ${JSON.stringify(join(root, "web/src/components/reader/History.svelte"))};
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};
const fixtures = ${JSON.stringify(fixtures)};

// What the panel asks for, in the order it asks.
const calls = [];
const callbacks = {
  onview: (sha) => calls.push(["view", sha]),
  oncompare: (from, to) => calls.push(["compare", from, to]),
  onname: (sha, label) => calls.push(["name", sha, label]),
  onretry: () => calls.push(["retry"]),
  onloadmore: () => calls.push(["loadmore"]),
  onloaduntil: (iso) => { calls.push(["loaduntil", iso]); return Promise.resolve(); },
  onloadsaved: () => calls.push(["loadsaved"]),
  onauthorsview: (on) => calls.push(["authorsview", on]),
  onisolate: (key) => calls.push(["isolate", key]),
};
const base = {
  labels: [], viewing: null, older: "", newer: "", canEdit: false, problem: "", loaded: true,
  complete: true, loadingMore: false, moreProblem: "", saved: [], savedLoaded: false,
  savedLoading: false, savedProblem: "",
};

let component = null;
const mount = (props) => {
  if (component) component.$destroy();
  calls.length = 0;
  component = createClassComponent({
    component: History, target: document.body, props: { ...base, ...callbacks, ...props },
  });
};
const update = (props) => component.$set(props);
const unmount = () => {
  if (component) component.$destroy();
  component = null;
};
const flush = async () => {
  await tick();
  await new Promise((resolve) => setTimeout(resolve, 30));
  await tick();
};

window.historyPanelCheck = ${runChecks.toString()};
`;

writeFileSync(entry, source);
let server;
let tab;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [tailwindcss(), svelte()], logLevel: "error",
    build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "history-panel-check.js", cssFileName: "style" } } });
  assertFreshDist("history-panel-browser");
  // The built shell's stylesheet carries the theme -- every colour and step
  // of spacing the panel uses is a custom property defined there.
  const built = join(root, "web/dist/assets");
  const theme = readdirSync(built)
    .filter((name) => name.endsWith(".css"))
    .map((name) => readFileSync(join(built, name), "utf8"))
    .filter((text) => text.includes("--color-sidebar"))
    .join("\n");
  if (!theme) throw new Error("no built theme stylesheet: run `bun run build` in web/ first");
  server = createServer((request, response) => {
    if (request.url === "/theme.css") {
      response.setHeader("Content-Type", "text/css");
      return response.end(theme);
    }
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) {
      response.setHeader("Content-Type", request.url.endsWith(".css") ? "text/css" : "text/javascript");
      response.end(readFileSync(file));
      return;
    }
    response.setHeader("Content-Type", "text/html");
    response.end('<!doctype html><html data-theme="librepaper"><head>'
      + '<link rel="stylesheet" href="/theme.css"><link rel="stylesheet" href="/style.css">'
      + '<style>body{margin:0;width:340px}</style></head>'
      + '<body><script type="module" src="/history-panel-check.js"></script></body></html>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  tab = await browser("chromium", profile, 22000 + Math.floor(Math.random() * 1000));
  await tab.navigate(`http://127.0.0.1:${port}/`);
  await until("history panel component", () => tab.evaluate("Boolean(window.historyPanelCheck)"));
  const result = await tab.evaluate("window.historyPanelCheck()");
  assert.equal(result, "passed", "history panel browser check");
  console.log("history panel: events fold by session, pushes and named versions, periods, saved view, layout, keyboard");
} finally {
  await tab?.close(); server?.close(); removeTemporary(temporary);
}
