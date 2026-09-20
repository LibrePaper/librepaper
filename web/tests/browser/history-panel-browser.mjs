// Browser regression check for the real history panel: three views of one
// past -- the day's versions, a month that chooses another day, and the
// bookmarks somebody left. A day is coarsened by significance: a version
// somebody asked for is always its own row, the autosaves between two of
// those are one row that says what the run of them changed, and either opens
// where it stands. Details belong to the chosen version and to no other, and
// what was written and never saved is folded away behind a single line.
//
// The volume is the point of all of it: the server saves a version after
// thirty seconds of quiet, so the fixtures here are small but the arrangement
// they check is the one that has to survive two hundred of them in a day.
//
// The panel reads days in the reader's own timezone, so the fixture below
// would fall on different days in different ones. The browser is launched in
// UTC so that the days it draws are the days written here.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { readdirSync } from "node:fs";
import { browser, until } from "../../tools/browser-driver.mjs";

process.env.TZ = "UTC";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-history-panel-check-"));
const output = join(temporary, "build");
const profile = join(temporary, "browser");
const entry = join(temporary, "entry.js");
const points = Array.from({ length: 5 }, (_, index) => ({
  sha: String(index + 1).padStart(64, "0"),
  at: index === 0 ? "2026-09-09T09:00:00Z" : `2026-09-10T09:0${index}:00Z`,
  by: "Vincent",
  why: "quiet",
  label: index === 4 ? "Submitted draft" : index === 0 ? "Earlier draft" : "",
  changed: ["main.md"],
  parent: index ? String(index).padStart(64, "0") : undefined,
}));
const otherFilePoint = {
  sha: "9".padStart(64, "0"), at: "2026-09-11T09:00:00Z", by: "Vincent",
  why: "quiet", label: "", changed: ["references.bib"], parent: points[4].sha,
};

// The three answers the manifest gives about what a version moved, which the
// row has to tell apart: named paths, an answered "no file", and no answer at
// all. The last is every version written before the catalogue recorded one.
const movedPoints = [
  { sha: "a".repeat(64), at: "2026-09-11T10:00:00Z", by: "Vincent", why: "quiet", label: "",
    changed: ["chapters/intro.tex", "references.bib", "figures/plot.png"] },
  { sha: "b".repeat(64), at: "2026-09-11T10:00:00Z", by: "Vincent", why: "quiet", label: "",
    changed: [] },
  { sha: "c".repeat(64), at: "2026-09-11T10:02:00Z", by: "Vincent", why: "quiet", label: "" },
  // A fourth, so the ten o'clock sitting holds more versions than the panel
  // will list and has to offer its own axis instead.
  { sha: "e".repeat(64), at: "2026-09-11T10:03:00Z", by: "Vincent", why: "quiet", label: "",
    changed: ["main.md"] },
];

// A version three months before the rest, used to check that the calendar can
// be walked back to it and stops there. It is set on the panel partway through
// rather than mounted with it, so the months either side of the document are
// tested without depending on what today happens to be.
const earlier = {
  sha: "d".repeat(64), at: "2026-06-15T12:00:00Z", by: "Vincent", why: "quiet", label: "",
};

// One version somebody asked for, set on the panel partway through, so that
// a row with a word on it can be told from the rows without one.
const published = {
  sha: "f".repeat(64), at: "2026-09-11T10:20:00Z", by: "Vincent", why: "cli", label: "",
  changed: ["main.md"],
};

// Thirty autosaves a minute apart, half from each of two people, on a day too
// big to list -- so whether they are one run or two is entirely the question
// of whether a different person at the keyboard ends one.
const shared = Array.from({ length: 30 }, (_, index) => ({
  sha: String(index).padStart(64, "7"),
  at: `2026-09-12T11:${String(index).padStart(2, "0")}:00Z`,
  by: index % 2 ? "Anne" : "Vincent",
  why: "quiet", label: "", changed: ["main.md"],
}));

// An afternoon of the kind the server actually produces: ninety autosaves,
// a couple of minutes apart, with real pauses scattered through. Far past
// what anybody reads as a list, and the day the coarsening exists for.
const busyDay = Array.from({ length: 90 }, (_, index) => {
  const minute = 540 + index * 2 + (index % 17 === 0 ? 6 : 0);
  return {
    sha: String(index).padStart(64, "b"),
    at: `2026-09-13T${String(Math.floor(minute / 60)).padStart(2, "0")}:${String(minute % 60).padStart(2, "0")}:00Z`,
    by: "Vincent", why: "quiet", label: "", changed: ["main.tex"],
  };
});

const source = `
import History from ${JSON.stringify(join(root, "web/src/components/reader/History.svelte"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};
const points = ${JSON.stringify(points)};
const otherFilePoint = ${JSON.stringify(otherFilePoint)};
const movedPoints = ${JSON.stringify(movedPoints)};
const earlier = ${JSON.stringify(earlier)};
const published = ${JSON.stringify(published)};
const shared = ${JSON.stringify(shared)};
const busyDay = ${JSON.stringify(busyDay)};
const all = [...points, otherFilePoint, ...movedPoints];
const events = [];
const component = createClassComponent({ component: History, target: document.body, props: {
  labels: all, canEdit: true, currentLabel: "",
  onview: (sha) => events.push(["view", sha]),
  onname: (sha, label) => events.push(["name", sha, label]),
} });
const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 30)); await tick(); };
const check = (condition, message) => { if (!condition) throw new Error(message); };
window.historyPanelCheck = async () => {
  await flush();
  // The three views, and the one control that chooses between them.
  const strip = () => [...document.querySelectorAll('.history-tabs [role="tab"]')];
  const show = async (name) => { document.querySelector('#history-tab-' + name).click(); await flush(); };
  const showing = () => document.querySelector('.history-tabs [aria-selected="true"]')?.textContent.trim();
  const cells = () => [...document.querySelectorAll('[data-history-day]')];
  const cell = (day) => cells().find((node) => node.dataset.historyDay === day);
  const list = () => [...document.querySelectorAll('.day-row:not(.day-now)')];
  const nowRow = () => document.querySelector('.day-row.day-now');
  const runs = () => [...document.querySelectorAll('.day-run')];
  const text = (node) => node?.textContent.replace(/\\s+/g, ' ').trim() ?? null;
  // What a row puts in front of the eye, which is not all a row says: the
  // word an autosave withholds from the page it still owes a screen reader.
  const seen = (node) =>
    [...node.querySelectorAll('.day-when, .day-what, .now-label, .now-what')]
      .map(text).filter(Boolean).join(' ');

  /* ----------------------------------- a day that fits is a day that is listed */

  // The panel opens on a day, with the month behind its own tab rather than
  // a grid beside it or a line above it. Five versions is a day anybody can
  // read, so every one of them is a row -- and this is the regression guard: coarsening a day of five
  // into one entry standing for them saved nobody anything and left a panel
  // whose only row could not be chosen, which is the one thing it is for.
  check(strip().map((node) => node.textContent.trim()).join() === 'Timeline,Calendar,Bookmarks',
    'three views, in the strip every tabbed panel wears');
  check(showing() === 'Timeline', 'and it opens on the day, which is what a version history is opened for');
  check(!cells().length, 'the month must not be drawn beside the day');
  check(!document.querySelector('.crumb, .crumb-day, .crumb-back'),
    'and no line above it naming the day: the Calendar tab is where a day is chosen');
  check(list().length === 5 && !runs().length,
    'a day that fits is listed, never gathered: '
    + list().length + ' rows, ' + runs().length + ' runs');
  check(!document.querySelector('.track, .sitting, .spark, .day-bar, .day-dot, .day-leader'),
    'and nothing else: no spine, no bars, no sparklines, no markers');
  const said = list().map(seen);
  // Newest first: a version history is read from its end, and the last thing
  // somebody did is what they nearly always came back for.
  check(said.join('|') === '10:03 AM|10:02 AM|10:00 AM|10:00 AM|9:00 AM',
    'each of them its time and nothing more, latest first: ' + said.join('|'));
  // The live document is the head of that list rather than a banner above the
  // month: it is what every version is compared against, and nothing is newer.
  check(nowRow() && document.querySelector('.day-row') === nowRow(),
    'the current version is the first row of the list');
  // Its own shape, though: a label and what it points at, at the two ends of
  // the row. It is not an event, and giving it the columns the events use
  // said it was one.
  check(seen(nowRow()) === 'Now Current version',
    'and says what it is: ' + seen(nowRow()));
  check(nowRow().querySelector('.now-point') && !nowRow().querySelector('.day-when'),
    'without borrowing the columns the events below it are laid out in');
  check(!document.querySelector('.now, .timeline-now'),
    'with nothing left of the banner it used to be');
  // It still has to name itself to a screen reader and to a tooltip.
  const first = list()[0].querySelector('.timeline-point');
  check(text(first.querySelector('.sr-only')) === 'Synced'
    && first.title.includes('Synced'),
    'what it was is said where saying it costs the eye nothing');
  // And one somebody asked for says which, wherever it appears.
  component.$set({ labels: [...all, published] }); await flush();
  // "Saved", not "Published": nobody publishes anything any more, and the
  // word outlived the act by a release.
  check(seen(list()[0]) === '10:20 AM Saved',
    'a version somebody asked for says which it was: ' + seen(list()[0]));
  component.$set({ labels: all }); await flush();

  /* ------------------------------ a day too big to read is coarsened */

  // Ninety versions in one afternoon is what the server actually produces --
  // it saves after thirty seconds of quiet -- and that is the day the
  // coarsening exists for.
  component.$set({ labels: busyDay }); await flush();
  check(!list().length && runs().length > 1,
    'a day too big to read is its runs: ' + runs().length + ' runs, ' + list().length + ' rows');
  check(runs().length < 10, 'a handful of them, not ninety: ' + runs().length);
  const heads = () => [...document.querySelectorAll('.run-head')];
  const stands = heads().map((node) => Number(text(node.querySelector('.run-what')).split(' ')[0]));
  check(stands.every((count) => count <= 25),
    'and no one of them stands for more than a screenful: ' + stands.join());
  check(stands.reduce((sum, count) => sum + count, 0) === busyDay.length,
    'with none of them dropped: ' + stands.join());
  check(text(heads()[0].querySelector('.run-what')).endsWith('· main.tex'),
    'a run says what the whole of it changed: ' + text(heads()[0].querySelector('.run-what')));
  check(/\\d.*–.*\\d/.test(text(heads()[0].querySelector('.run-when'))),
    'and when it ran: ' + text(heads()[0].querySelector('.run-when')));

  // Opened, a run is the versions in it -- where it stands, with the rest of
  // the day still below rather than replaced by it. That is how a version
  // inside one is reached, and reaching it is the point.
  check(heads()[0].getAttribute('aria-expanded') === 'false');
  heads()[0].click(); await flush();
  check(heads()[0].getAttribute('aria-expanded') === 'true');
  check(list().length === stands[0], 'opened, a run is the versions in it: ' + list().length);
  check(document.querySelector('.day-run .day-row'), 'shown where the run stands');
  const inside = list()[0].dataset.sha;
  list()[0].querySelector('.timeline-point').click(); await flush();
  check(events.some((event) => event[0] === 'view' && event[1] === inside),
    'and choosing one asks for its comparison');
  heads()[0].click(); await flush();
  check(!list().length, 'and it closes again');

  // A version chosen from outside a closed run opens the run it is in: a
  // selection nobody can see is worse than none.
  component.$set({ viewing: inside }); await flush();
  check(document.querySelector('[data-sha="' + inside + '"]'),
    'a version chosen from inside a closed run opens the run it is in');
  component.$set({ viewing: '' }); await flush();

  // Two people writing at once is one run, and it credits neither of them.
  // An autosave is attributed to whoever sent the last update before it
  // fired, which on a shared document is a coin toss -- so the panel never
  // turns that name into a claim about whose work a stretch of it was.
  component.$set({ labels: shared }); await flush();
  const hands = [...document.querySelectorAll('.run-what')].map(text);
  // Two, because thirty is past the ceiling -- not thirty, which is what
  // splitting on the name would have given.
  check(hands.length === 2 && hands.every((one) => /^15 autosaves · main\\.md$/.test(one)),
    'thirty alternating authors are cut by the ceiling, never by the name: '
    + hands.join(' | '));
  check(!hands.some((one) => /Anne|Vincent/.test(one)), 'and no run credits either of them');
  component.$set({ labels: all }); await flush();

  /* ----------------------------------------------- one version at a time */

  check(!document.querySelector('.day-about'), 'no version shows its details until it is chosen');
  // The one control a row does carry is the bookmark, and an unmarked one
  // does not offer itself until the row is under the pointer, has the
  // keyboard, or is the one being read.
  check(!document.querySelector('.day-tools.on'), 'no version is marked to begin with');
  check([...document.querySelectorAll('.day-row:not(.day-now) .day-tools')]
    .every((node) => getComputedStyle(node).opacity === '0'),
    'and an unmarked row keeps its bookmark out of the way');
  check(!document.querySelector('[aria-label="Rename this bookmark"], [aria-label="Remove this bookmark"]'),
    'renaming and removing belong to the bookmarks, not to the timeline');
  const point = (sha) => document.querySelector('[data-sha="' + sha + '"] .timeline-point');
  point(movedPoints[0].sha).click(); await flush();
  check(events.some((event) => event[0] === 'view' && event[1] === movedPoints[0].sha),
    'selecting a version asks for its comparison');
  component.$set({ viewing: movedPoints[0].sha }); await flush();
  const card = () => document.querySelector('.day-about');
  check(card() && document.querySelectorAll('.day-about').length === 1,
    'the chosen version, and only that one, shows its details');
  // Inside the row it belongs to, rather than in a third row underneath it.
  check(document.querySelector('.day-row .timeline-here .day-about'),
    'on the row itself');
  check(/^\\d.*3 files changed$/.test(text(card())),
    'an autosave says its exact time and what it moved, and claims no author: ' + text(card()));
  component.$set({ labels: [...all, published], viewing: published.sha }); await flush();
  check(text(card()).startsWith('Vincent · '),
    'a version somebody asked for names the person who asked: ' + text(card()));
  component.$set({ labels: all }); await flush();
  component.$set({ viewing: movedPoints[0].sha }); await flush();
  const tools = () => document.querySelector('.day-row:not(.day-now):has(.timeline-here) .day-tools');
  check(tools() && tools().querySelector('[aria-label="Bookmark this version"]')
    && getComputedStyle(tools()).opacity === '1',
    'the row being read shows the one thing that can be done to it from here');
  check(!document.querySelector('.day-tools a, .day-tools [aria-label*="link" i]'),
    'and nothing that offers a link to a version: the pane is not where links come from');
  check(!document.querySelector('[aria-label="Restore this version"]'),
    'restoring belongs to the comparison, where what it replaces is on the screen');
  component.$set({ viewing: movedPoints[1].sha }); await flush();
  check(text(card()).includes('No files changed'));
  component.$set({ viewing: movedPoints[2].sha }); await flush();
  check(!text(card()).includes('file'),
    'a version that cannot account for itself does not claim to');
  component.$set({ viewing: otherFilePoint.sha }); await flush();
  check(text(card()).includes('references.bib'),
    'one moved file is named, without its directory');
  component.$set({ viewing: '' }); await flush();

  // Nothing below the versions: the minutes nobody saved are not listed.
  // A clock time cannot say which minute is the one wanted, and the row that
  // used to count them said nothing a reader could act on.
  check(!document.querySelector(".unsaved, .unsaved-more, .unsaved-row"),
    "what was never saved is not on the timeline");

  /* ------------------------------------------------ over to the month */

  await show('calendar');
  check(showing() === 'Calendar', 'the month is where a reader looking for another day goes');
  check(!list().length && !runs().length, 'the day stands down when the month comes up');
  check(cells().length % 7 === 0 && cells().length > 28, 'the month is whole weeks');
  check(cell('2026-09-01') && cell('2026-09-30'), 'and the whole month');
  // September 2026 begins on a Tuesday, so the grid runs on into August and
  // October rather than leaving holes in its corners.
  check(cell('2026-08-30') && cell('2026-10-03'), 'running on into the months either side');
  check(cell('2026-08-30').classList.contains('cal-outside'));
  check(cell('2026-09-16').querySelector('.cal-date').textContent === '16',
    'a cell carries its own date, because a month is counted across by date');
  const count = (day) => cell(day)?.querySelector('.cal-count')?.textContent ?? null;
  check(count('2026-09-09') === '1' && count('2026-09-10') === '4' && count('2026-09-11') === '5',
    'and the number of versions that landed on it');
  check(count('2026-09-08') === null, 'a day nothing happened on carries no number');
  check(cell('2026-09-11').getAttribute('aria-pressed') === 'true',
    'the day that was open is the day the month opens on');

  // Two arrows, and no more: the month steps, and stops at the ends of the
  // document rather than wandering into years nothing happened in.
  check(document.querySelectorAll('.cal-head button').length === 3,
    'the month header is two arrows and the month itself');
  const prev = () => document.querySelector('[aria-label="Previous month"]');
  check(prev().disabled, 'September is the first month this document has');
  component.$set({ labels: [earlier, ...all] }); await flush();
  check(!prev().disabled, 'a version in June is a June to walk back to');
  prev().click(); await flush();
  check(document.querySelector('.cal-month').textContent.trim() === 'August 2026',
    'the arrow steps a month back');
  prev().click(); prev().click(); await flush();
  check(document.querySelector('.cal-month').textContent.trim() === 'June 2026'
    && count('2026-06-15') === '1', 'and keeps stepping, to the month the earliest version is in');
  check(prev().disabled, 'where it stops');
  document.querySelector('[aria-label="Next month"]').click(); await flush();
  check(document.querySelector('.cal-month').textContent.trim() === 'July 2026');

  /* ----------------------------------------------------- the bookmarks */

  await show('bookmarks');
  check(!document.querySelector('.cal-grid') && !list().length,
    'a view at a time: the month and the day stand down for the marks');
  const marks = () => [...document.querySelectorAll('.mark-name')].map((node) => node.textContent);
  // Latest first, as every list in this panel is.
  check(marks().join() === 'Submitted draft,Earlier draft',
    'the bookmarks are the one way in that is not a date: ' + marks().join());
  const markRow = (name) => [...document.querySelectorAll('.mark-row')]
    .find((node) => node.querySelector('.mark-name')?.textContent === name);
  // Rename and remove live here, on the row, and nowhere else.
  check(markRow('Earlier draft').querySelector('[aria-label="Rename this bookmark"]')
    && markRow('Earlier draft').querySelector('[aria-label="Remove this bookmark"]'),
    'each carries the two things that can be done to a name');
  markRow('Earlier draft').querySelector('[aria-label="Rename this bookmark"]').click(); await flush();
  const rename = document.querySelector('[aria-label="Name this bookmark"]');
  check(rename && rename.tagName === 'INPUT' && rename.value === 'Earlier draft',
    'renaming opens on the name it already has');
  rename.value = 'First draft'; rename.dispatchEvent(new Event('input', { bubbles: true }));
  rename.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); await flush();
  check(events.some((event) => event[0] === 'name' && event[1] === points[0].sha && event[2] === 'First draft'),
    'and reports the new one');
  markRow('Earlier draft').querySelector('[aria-label="Remove this bookmark"]').click(); await flush();
  check(events.some((event) => event[0] === 'name' && event[1] === points[0].sha && event[2] === ''),
    'removing a bookmark takes the name off the version, and does not touch the version');
  document.querySelector('.mark-point').click(); await flush();
  check(showing() === 'Timeline' && (list().length || runs().length),
    'opening one drills straight into its day');
  check(events.some((event) => event[0] === 'view' && event[1] === points[4].sha),
    'and asks for its comparison');
  await show('calendar');
  check(cell('2026-09-10').getAttribute('aria-pressed') === 'true',
    'and the day it drilled into is that version’s own day');

  /* ------------------------------------------------- marking a version */

  await show('timeline');
  component.$set({ viewing: '' }); await flush();
  const unmarked = list().find((node) => node.querySelector('[aria-label="Bookmark this version"]'));
  check(unmarked, 'an unmarked version offers the mark');
  unmarked.querySelector('[aria-label="Bookmark this version"]').click(); await flush();
  const naming = document.querySelector('[aria-label="Name this bookmark"]');
  // It arrives with a name rather than empty, because an empty field is a
  // question and most bookmarks are worth having under any name at all. The
  // default says which version this is, and it is selected, so typing over it
  // costs nothing.
  check(naming && naming.tagName === 'INPUT' && naming.value.startsWith('Draft of ')
    && naming.value.includes('2026'),
    'marking one offers a name it can be kept under: ' + (naming && naming.value));
  check(naming.selectionStart === 0 && naming.selectionEnd === naming.value.length,
    'and offers it selected, so the first key typed replaces it');
  naming.value = 'Sent to the journal'; naming.dispatchEvent(new Event('input', { bubbles: true }));
  naming.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); await flush();
  check(events.some((event) => event[0] === 'name' && event[1] === unmarked.dataset.sha
    && event[2] === 'Sent to the journal'), 'and the name is what makes it a bookmark');
  // A version that already carries one says so without being hovered, and
  // offers the rename rather than a second mark.
  const marked = () => document.querySelector('[data-sha="' + points[4].sha + '"]');
  check(marked() && marked().querySelector('.day-tools.on')
    && getComputedStyle(marked().querySelector('.day-tools')).opacity === '1'
    && marked().querySelector('[aria-label="Rename this bookmark"]'),
    'a marked version wears its mark, and the mark does not hide');

  /* -------------------------------------------------------- the present */

  component.$set({ viewing: '' }); await flush();
  await show('calendar');
  // The live document is a row in the day, so the month -- which is a day
  // picker and nothing else -- does not carry it.
  check(!nowRow(), 'the month is days, and the present is not one of them');
  cell('2026-09-11').click(); await flush();
  check(showing() === 'Timeline' && nowRow(),
    'and it is back at the head of the list the moment a day is open');
  const currentName = document.querySelector('[aria-label="Bookmark the current version"]');
  check(currentName, 'the current version can be bookmarked like any other'); currentName.click(); await flush();
  const input = document.querySelector('[aria-label="Name this bookmark"]');
  check(input && input.tagName === 'INPUT', 'current naming uses an accessible input');
  input.value = 'Working draft'; input.dispatchEvent(new Event('input', { bubbles: true }));
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); await flush();
  check(events.some((event) => event[0] === 'name' && event[1] === 'current' && event[2] === 'Working draft'),
    'current naming reports the current pseudo-version');
  component.$destroy();
  return true;
};
`;

writeFileSync(entry, source);
let server;
let tab;
try {
  await build({ configFile: false, root: join(root, "web"), plugins: [svelte()], logLevel: "error",
    build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "history-panel-check.js" } } });
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
  assert.equal(await tab.evaluate("window.historyPanelCheck()"), true);
  console.log("history panel: three views, a day coarsened by significance, and bookmarks");
} finally {
  await tab?.close(); server?.close(); rmSync(temporary, { recursive: true, force: true });
}
