// Browser regression check for the real history panel, which narrows three
// times and never shows two of the steps at once: a month that chooses a day,
// a day that is the sittings somebody had rather than twenty-four hours of
// mostly nothing, and -- only for a sitting too busy to list -- that one
// interval on a real axis of minutes. Details belong to the chosen version
// and to no other.
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

// The writes the activity strip is drawn from: four minutes falling in three
// ten-minute bins, the last of them the busiest, so the bars can be checked
// for where they are and for how long they are.
// The writes the sittings are cut from. Nine o'clock to twenty past is one
// sitting; forty minutes of nothing, then ten o'clock is another. The panel
// draws no height at all for the forty minutes.
const rows = [
  { at: "2026-09-11T09:02:00Z", peer: "", changes: 1, state_bytes: 100, frontier: "one" },
  { at: "2026-09-11T09:20:00Z", peer: "", changes: 3, state_bytes: 300, frontier: "two" },
  { at: "2026-09-11T10:01:00Z", peer: "", changes: 3, state_bytes: 700, frontier: "three" },
  { at: "2026-09-11T10:04:00Z", peer: "", changes: 4, state_bytes: 1500, frontier: "four" },
];

const source = `
import History from ${JSON.stringify(join(root, "web/src/components/reader/History.svelte"))};
import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};
const points = ${JSON.stringify(points)};
const otherFilePoint = ${JSON.stringify(otherFilePoint)};
const movedPoints = ${JSON.stringify(movedPoints)};
const earlier = ${JSON.stringify(earlier)};
const rows = ${JSON.stringify(rows)};
const all = [...points, otherFilePoint, ...movedPoints];
const events = [];
const component = createClassComponent({ component: History, target: document.body, props: {
  checkpoints: all, activity: rows, canEdit: true, currentLabel: "",
  onview: (sha) => events.push(["view", sha]),
  onname: (sha, label) => events.push(["name", sha, label]),
} });
const flush = async () => { await tick(); await new Promise((resolve) => setTimeout(resolve, 30)); await tick(); };
const check = (condition, message) => { if (!condition) throw new Error(message); };
window.historyPanelCheck = async () => {
  await flush();
  const crumb = () => document.querySelector('.crumb-back')?.textContent.trim() ?? null;
  const cells = () => [...document.querySelectorAll('[data-history-day]')];
  const cell = (day) => cells().find((node) => node.dataset.historyDay === day);
  const sittings = () => [...document.querySelectorAll('.sitting')];
  const text = (node) => node?.textContent.replace(/\\s+/g, ' ').trim() ?? null;
  const shas = () => [...document.querySelectorAll('[data-sha]')].map((node) => node.dataset.sha);

  /* --------------------------------------------- one step at a time */

  // The panel opens on the day's sittings. The month is a way back, not a
  // grid beside them, and no clock is drawn at all until a sitting is
  // opened: showing two of the three levels at once is the thing this
  // arrangement exists to stop.
  check(!cells().length, 'the month must not be drawn beside the day');
  check(!document.querySelector('.axis'), 'nor an axis until a sitting asks for one');
  check(crumb() === '‹ September 2026', 'the month is one click above');

  /* --------------------------------------- the day is sittings, not hours */

  // 09:00 and 09:02 are one sitting; 09:35 is the same one, twenty-five
  // minutes being the longest pause that still counts; 10:00 onwards is
  // another. Nothing is drawn for the fifty minutes nobody worked.
  check(sittings().length === 2, 'the day is the sittings somebody had');
  // The exact spelling of a time is the reader's locale's business; that it
  // is the sitting's own start and end is this panel's.
  const when = text(document.querySelector('.sitting-when'));
  check(when && when.includes('9:00') && when.includes('9:20'),
    'a sitting is named by when it ran, and it ran from 9:00 to 9:20: ' + when);
  const gap = document.querySelector('.feed-gap');
  check(text(gap) === '40m later',
    'and the silence before the next one is a sentence, not empty space');
  check(gap.getBoundingClientRect().height < 30,
    'forty minutes of nothing costs one line, whatever it would cost in proportion');

  // The sparse sitting lists its versions where they are; the dense one
  // offers its own axis instead of a column of near-identical rows.
  const first = sittings()[0];
  const second = sittings()[1];
  check(first.querySelectorAll('[data-sha]').length === 1,
    'three versions or fewer are simply listed');
  check(text(first).includes('4 writes') && text(first).includes('1 version'),
    'under a heading that counts both what was written and what was saved');
  check(first.querySelector('.spark') && first.querySelectorAll('.spark-bar').length === 24,
    'with a fixed-width sparkline for the shape of it');
  check(!second.querySelectorAll('[data-sha]').length,
    'a sitting with more versions than that lists none of them');
  check(second.querySelector('.sitting-open'), 'and offers to open itself instead');
  check(text(second).includes('4 versions'));
  check(second.getBoundingClientRect().height < 90,
    'and a sitting is a few lines whether it ran for two minutes or two hours');

  /* ------------------------------------- one sitting, on a real axis */

  second.querySelector('.sitting-open').click(); await flush();
  check(!sittings().length, 'the day stands down when one of its sittings is opened');
  check(document.querySelector('.axis'), 'and that sitting gets an axis of its own');
  check(crumb().startsWith('‹'), 'with a way back to the day');
  check(shas().length === 4, 'every version in the sitting is on it');
  // The window is the sitting, not the day: a quarter of an hour of axis
  // rather than twenty-four hours of mostly nothing.
  const labels = [...document.querySelectorAll('.axis-hour')].map((node) => node.textContent);
  check(labels.length >= 2 && labels.length <= 12,
    'the axis is ticked for the interval it covers, not for a whole day: ' + labels.join());
  check(!labels.includes('3:00 AM') && !labels.includes('11:00 PM'),
    'no hour outside the sitting is drawn at all');

  // Position is time. A dot is at its own minute whatever is near it, and a
  // label that would collide is pushed clear with a leader back to its dot.
  const at = (node) => Number(node.style.top.replace('px', ''));
  const dots = [...document.querySelectorAll('.axis-canvas > .day-dot')];
  check(dots.length === 4, 'one dot per version, and only one');
  check(at(dots[0]) === at(dots[1]),
    'two versions in the same minute are in the same place, because place is time');
  check(at(dots[2]) > at(dots[1]) && at(dots[3]) > at(dots[2]), 'the axis runs forwards');
  const minute = at(dots[3]) - at(dots[2]);
  check(minute > 2 && Math.abs((at(dots[2]) - at(dots[1])) - 2 * minute) < 0.6,
    'and two minutes is twice as far as one: the axis is proportional, not merely ordered');
  // The dots cannot both be at ten o'clock and both be readable, so the
  // second label is pushed clear -- and keeps a leader back to its own dot.
  const marks = [...document.querySelectorAll('.axis-mark')];
  check(at(marks[1]) > at(dots[1]) + 4, 'a label that would collide is pushed clear');
  check(document.querySelectorAll('.day-leader').length >= 1,
    'and keeps a leader back to the dot it belongs to');
  check(!document.querySelector('.hour'), 'no row per hour survives anywhere');

  // The writes are bars beside the same axis, never rows of their own.
  const bars = [...document.querySelectorAll('.day-bar')];
  check(bars.length === 2, 'the writes in the sitting are bars, not events');
  check(bars.every((node) => node.offsetWidth <= 48),
    'and a bar stays inside its strip rather than becoming a rule across the panel');
  check(Number(bars[0].dataset.minute) === 601 && Number(bars[1].dataset.minute) === 604);

  /* ----------------------------------------------- one version at a time */

  check(!document.querySelector('.day-card'), 'no version shows its details until it is chosen');
  check(!document.querySelector('[aria-label="Name this version"]'),
    'unselected revisions do not expose row actions');
  const point = (sha) => document.querySelector('[data-sha="' + sha + '"] .timeline-point');
  point(movedPoints[0].sha).click(); await flush();
  check(events.some((event) => event[0] === 'view' && event[1] === movedPoints[0].sha),
    'selecting a version asks for its comparison');
  component.$set({ viewing: movedPoints[0].sha }); await flush();
  const card = document.querySelector('.day-card');
  check(card && document.querySelectorAll('.day-card').length === 1,
    'the chosen version, and only that one, shows its details');
  check(card.textContent.includes('3 files changed'),
    'what it moved is in the card rather than on the axis');
  check(card.querySelector('[aria-label="Name this version"]')
    && card.querySelector('[aria-label="Copy the link to this version"]'),
    'with the actions that belong to it');
  check(!document.querySelector('[aria-label="Restore this version"]'),
    'restoring belongs to the comparison, where what it replaces is on the screen');
  component.$set({ viewing: movedPoints[1].sha }); await flush();
  check(document.querySelector('.day-card').textContent.includes('No files changed'));
  component.$set({ viewing: movedPoints[2].sha }); await flush();
  check(!document.querySelector('.day-card').textContent.includes('file'),
    'a version that cannot account for itself does not claim to');
  // A version from another sitting entirely: the panel steps back up to the
  // day rather than leaving the reader on a sitting that does not hold what
  // they are comparing.
  component.$set({ viewing: otherFilePoint.sha }); await flush();
  check(!document.querySelector('.axis') && sittings().length === 2,
    'choosing a version outside the open sitting steps back up to the day');
  check(document.querySelector('.day-card').textContent.includes('references.bib'),
    'one moved file is named, without its directory');
  component.$set({ viewing: '' }); await flush();

  /* ------------------------------------------------ back up the steps */

  document.querySelector('.crumb-back').click(); await flush();
  check(!document.querySelector('.feed'), 'and the day stands down when the month does');
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
  // A day is a date on the panel's ground, not a filled box: the mark under
  // it is the activity, and a day nobody touched draws nothing at all.
  const activity_ = (day) => cell(day)?.querySelector('.cal-mark');
  check(activity_('2026-09-10')?.dataset.level === '4',
    'a day is marked by how much was written on it');
  check(activity_('2026-09-09')?.dataset.level === '1');
  check(!activity_('2026-09-08'), 'and a day nobody touched is marked not at all');
  check(cell('2026-09-08').classList.contains('cal-quiet'), 'it recedes instead');
  check(cell('2026-09-11').getAttribute('aria-pressed') === 'true',
    'the day that was open is the day the month opens on');

  // Two arrows, and no more: the month steps, and stops at the ends of the
  // document rather than wandering into years nothing happened in.
  check(document.querySelectorAll('.cal-head button').length === 3,
    'the month header is two arrows and the month itself');
  const prev = () => document.querySelector('[aria-label="Previous month"]');
  check(prev().disabled, 'September is the first month this document has');
  component.$set({ checkpoints: [earlier, ...all] }); await flush();
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

  /* --------------------------------------------------------- the names */

  const names = [...document.querySelectorAll('.names-row strong')].map((node) => node.textContent);
  check(names.join() === 'Earlier draft,Submitted draft',
    'the named versions are the one way in that is not a date');
  document.querySelectorAll('.names-row')[0].click(); await flush();
  check(!document.querySelector('.cal-grid') && document.querySelector('.feed'),
    'opening one drills straight into its day');
  check(events.some((event) => event[0] === 'view' && event[1] === points[0].sha),
    'and asks for its comparison');
  document.querySelector('.crumb-back').click(); await flush();
  check(cell('2026-09-09').getAttribute('aria-pressed') === 'true',
    'and the day it drilled into is that version’s own day');

  /* -------------------------------------------------------- the present */

  component.$set({ viewing: '' }); await flush();
  const currentName = document.querySelector('[aria-label="Name the current version"]');
  check(currentName, 'current version has a naming action'); currentName.click(); await flush();
  const input = document.querySelector('[aria-label="Name the current version"]');
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
  server = createServer((request, response) => {
    const file = join(output, request.url.slice(1));
    if (request.url !== "/" && existsSync(file)) { response.setHeader("Content-Type", "text/javascript"); response.end(readFileSync(file)); return; }
    response.setHeader("Content-Type", "text/html"); response.end('<body><script type="module" src="/history-panel-check.js"></script></body>');
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  tab = await browser("chromium", profile, 22000 + Math.floor(Math.random() * 1000));
  await tab.navigate(`http://127.0.0.1:${port}/`);
  await until("history panel component", () => tab.evaluate("Boolean(window.historyPanelCheck)"));
  assert.equal(await tab.evaluate("window.historyPanelCheck()"), true);
  console.log("history panel: month, sittings, one sitting's minutes -- one step at a time");
} finally {
  await tab?.close(); server?.close(); rmSync(temporary, { recursive: true, force: true });
}
