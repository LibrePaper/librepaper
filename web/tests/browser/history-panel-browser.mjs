// Browser regression check for the real history panel: one list of days,
// newest first, with each day's versions folded away until somebody opens it.
// A day is coarsened by significance: a version somebody asked for is always
// its own row, the autosaves between two of those are one row that says what
// the run of them changed, and either opens where it stands. Details belong to
// the chosen version and to no other, and what was written and never saved is
// not listed. Bookmarks are a filter on the same list rather than a list of
// their own.
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
import { mkdtempSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { readdirSync } from "node:fs";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { assertFreshDist } from "../helpers/fresh-dist.mjs";

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

// A version three months before the rest, set on the panel partway through
// rather than mounted with it. It gives the list a day well before the others,
// which has to sit last and folded, whatever today's date happens to be.
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
  // The list is one column of days, and the filter that narrows it to the
  // bookmarks is the one control above it.
  const toggle = () => document.querySelector('[aria-label="Show only bookmarked versions"]');
  const filter = () => toggle()?.getAttribute('aria-pressed');
  const groups = () => [...document.querySelectorAll('ol.history-list > .day-group')];
  const days = () => groups().map((node) => node.dataset.day).join();
  const group = (day) => document.querySelector('.day-group[data-day="' + day + '"]');
  const head = (day) => group(day)?.querySelector('.day-head');
  const isOpen = (day) => head(day)?.getAttribute('aria-expanded') === 'true';
  const clickHead = async (day) => { head(day).click(); await flush(); };
  const rowsOf = (day) => [...(group(day)?.querySelectorAll('.day-row:not(.day-now)') ?? [])];
  const list = () => [...document.querySelectorAll('.day-row:not(.day-now)')];
  const nowRow = () => document.querySelector('.day-row.day-now');
  const runs = () => [...document.querySelectorAll('.day-run')];
  const text = (node) => node?.textContent.replace(/\\s+/g, ' ').trim() ?? null;
  const paragraph = (start) =>
    [...document.querySelectorAll('.panel-muted')].some((node) => text(node).startsWith(start));
  // What a row puts in front of the eye, which is not all a row says: the
  // word an autosave withholds from the page it still owes a screen reader.
  const seen = (node) =>
    [...node.querySelectorAll('.day-when, .day-what, .now-label, .now-what')]
      .map(text).filter(Boolean).join(' ');

  /* ----------------------------------- a day that fits is a day that is listed */

  // The panel opens on the latest day, with every other day folded away
  // beneath it. There is no month beside the list and no tab strip above it:
  // a version history is read from its newest day down.
  check(!document.querySelector('.history-tabs, [role="tab"], [data-history-day], .cal-grid'),
    'one list: no tabs, no month, no grid beside the days');
  check(toggle() && filter() === 'false',
    'the bookmarks are a filter on the list, and the list opens unfiltered');
  check(days() === '2026-09-11,2026-09-10,2026-09-09',
    'one group per day, newest first: ' + days());
  check(isOpen('2026-09-11') && !isOpen('2026-09-10') && !isOpen('2026-09-09'),
    'only the newest day starts open');
  check(!rowsOf('2026-09-10').length && !group('2026-09-10').querySelector('.day-run')
    && !rowsOf('2026-09-09').length && !group('2026-09-09').querySelector('.day-run'),
    'and a folded day holds no rows at all');
  check(text(group('2026-09-10').querySelector('.day-meta')) === '4 versions · main.md',
    'a folded day still says what it holds: ' + text(group('2026-09-10').querySelector('.day-meta')));
  const newest = text(head('2026-09-11').querySelector('.day-name'));
  check(newest.includes('Sep') && newest.includes('11'), 'and names its day: ' + newest);

  // Five versions is a day anybody can read, so every one of them is a row.
  // Coarsening a day of five into one entry standing for them saved nobody
  // anything and left a panel whose only row could not be chosen.
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
  // The live document is the head of that list rather than a banner above it:
  // it is what every version is compared against, and nothing is newer.
  check(nowRow() && document.querySelector('.day-row') === nowRow()
    && document.querySelector('ol.history-list').firstElementChild === nowRow(),
    'the current version is the first row of the list, and the first item in it');
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
  check(/\\d.*\\u2013.*\\d/.test(text(heads()[0].querySelector('.run-when'))),
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

  /* ------------------------------------------------ folding whole days */

  // A day opens on its own head, and its versions come with it while the
  // others stay as they were.
  await clickHead('2026-09-10');
  check(isOpen('2026-09-10') && rowsOf('2026-09-10').length === 4,
    'a folded day opens to its versions: ' + rowsOf('2026-09-10').length);
  check(isOpen('2026-09-11') && rowsOf('2026-09-11').length === 5,
    'and the newest day stays open beside it');
  await clickHead('2026-09-10');
  check(!isOpen('2026-09-10') && !rowsOf('2026-09-10').length,
    'and folds again, taking its rows with it');
  // The newest day folds like any other, and reopens the same way.
  await clickHead('2026-09-11');
  check(!isOpen('2026-09-11') && list().length === 0,
    'the newest day can be folded too, leaving no version rows: ' + list().length);
  await clickHead('2026-09-11');
  check(isOpen('2026-09-11') && rowsOf('2026-09-11').length === 5,
    'and it opens again');

  // A version chosen from a folded day opens that day, so its row is on the
  // screen and not somewhere below a closed head.
  component.$set({ viewing: points[2].sha }); await flush();
  check(isOpen('2026-09-10') && document.querySelector('[data-sha="' + points[2].sha + '"]'),
    'a version chosen from a folded day opens the day and shows its row');
  component.$set({ viewing: '' }); await flush();

  // A version from months back gets a day of its own at the end of the list,
  // folded like the rest, and nothing is drawn between it and the others.
  component.$set({ labels: [earlier, ...all] }); await flush();
  check(days() === '2026-09-11,2026-09-10,2026-09-09,2026-06-15',
    'a version from June is the last day of the list: ' + days());
  check(!isOpen('2026-06-15') && text(head('2026-06-15').querySelector('.day-name')).includes('Jun'),
    'folded, and named for its month: ' + text(head('2026-06-15').querySelector('.day-name')));
  component.$set({ labels: all }); await flush();

  /* ------------------------------------------- bookmarks as a filter */

  // The filter keeps only the days that hold a bookmark, opens them all, and
  // shows each bookmark as a row of its own. Runs are not part of it.
  toggle().click(); await flush();
  check(filter() === 'true', 'the toggle is pressed while the filter is on');
  check(!nowRow(), 'and the live document is not a bookmark, so it goes');
  check(days() === '2026-09-10,2026-09-09',
    'only the days holding a bookmark are listed: ' + days());
  check(groups().every((node) => isOpen(node.dataset.day)), 'and every one of them is open');
  const marks = () => [...document.querySelectorAll('.mark-name')].map((node) => node.textContent);
  // Latest first, as every list in this panel is.
  check(marks().join() === 'Submitted draft,Earlier draft',
    'the bookmarks are the one way in that is not a date: ' + marks().join());
  check(!runs().length, 'and no run is among them: a bookmark is a version of its own');
  // A day of bookmarks folds like any other day.
  head('2026-09-09').click(); await flush();
  check(!isOpen('2026-09-09') && marks().join() === 'Submitted draft',
    'a day of bookmarks folds on a click: ' + marks().join());
  head('2026-09-09').click(); await flush();
  check(isOpen('2026-09-09') && marks().length === 2, 'and opens again');
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
  check(events.some((event) => event[0] === 'view' && event[1] === points[4].sha),
    'clicking a bookmark asks for its comparison');

  // A filter with bookmarks in none of the versions it is given says so in a
  // sentence, rather than showing an empty list and leaving it at that.
  component.$set({ labels: busyDay }); await flush();
  check(filter() === 'true' && !document.querySelector('.mark-row') && paragraph('No bookmarks yet'),
    'with versions and no bookmarks, the filter says there are none yet');
  component.$set({ labels: all }); await flush();
  toggle().click(); await flush();
  check(filter() === 'false' && nowRow(),
    'the toggle takes the whole list back, and the present is at its head');

  /* ------------------------------------------------- marking a version */

  // The day of the bookmarked version is folded at the moment, so open it
  // before looking for the row inside it.
  if (!isOpen('2026-09-10')) await clickHead('2026-09-10');
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
  check(nowRow() && nowRow().querySelector('.now-point'),
    'the live document heads the list while the list is unfiltered');
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
  assert.equal(await tab.evaluate("window.historyPanelCheck()"), true);
  console.log("history panel: one list of days, coarsened by significance, bookmarks as a filter");
} finally {
  await tab?.close(); server?.close(); removeTemporary(temporary);
}
