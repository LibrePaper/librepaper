// Writes web/src/lib/symbols/emoji.json from Unicode's emoji test data and
// the CLDR English annotations. The palette reads the file; nothing at run
// time goes to the network.
//
// Run by hand when a new Emoji release ships. The output is committed, so a
// build never depends on these hosts being up.
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const EMOJI_TEST = "https://unicode.org/Public/emoji/latest/emoji-test.txt";
const CLDR_BASE = "https://raw.githubusercontent.com/unicode-org/cldr-json/main";
const CLDR_FILES = [
  `${CLDR_BASE}/cldr-json/cldr-annotations-full/annotations/en/annotations.json`,
  `${CLDR_BASE}/cldr-json/cldr-annotations-derived-full/annotationsDerived/en/annotations.json`,
];
const OUTPUT = fileURLToPath(new URL("../src/lib/symbols/emoji.json", import.meta.url));

// Skin tone modifiers and the hair components. Their sequences are dropped so
// the palette shows one tile per emoji rather than one per variant.
const skinTone = /[\u{1F3FB}-\u{1F3FF}]/u;
const hair = /[\u{1F9B0}-\u{1F9B3}]/u;

const download = async url => {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url} answered ${response.status}`);
  return response.text();
};

// CLDR keys are the emoji strings themselves. The derived file fills in
// sequences the full file does not cover, so the full file wins on conflict.
const annotations = new Map();
for (const url of CLDR_FILES.slice().reverse()) {
  const json = JSON.parse(await download(url));
  const table = json.annotations?.annotations ?? json.annotationsDerived?.annotations ?? {};
  for (const [glyph, entry] of Object.entries(table)) annotations.set(glyph, { ...annotations.get(glyph), ...entry });
}

const slugify = text => text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");

const groups = [];
const groupIds = new Map();
const emoji = [];
const slugs = new Set();
let group = null;

for (const line of (await download(EMOJI_TEST)).split("\n")) {
  const header = line.match(/^# group: (.+)$/);
  if (header) {
    const label = header[1].replace(/&/g, "and").trim();
    const id = slugify(label);
    if (!groupIds.has(id)) {
      groupIds.set(id, true);
      groups.push({ id, label });
    }
    group = id;
    continue;
  }
  if (line.startsWith("#") || !line.includes("; fully-qualified")) continue;
  const comment = line.match(/#\s*(\S+)\s+E\d+\.\d+\s+(.+)$/);
  if (!comment) continue;
  const [, glyph, emojiTestName] = comment;
  if (skinTone.test(glyph) || hair.test(glyph)) continue;
  const cldr = annotations.get(glyph) ?? {};
  // Source names occasionally use en or em dashes. Plain hyphens keep the
  // slugs and the search terms consistent with the rest of the data.
  const dashes = text => text.replace(/[\u2013\u2014]/g, "-");
  const name = dashes(cldr.tts?.[0] ?? emojiTestName).trim();
  let slug = slugify(name);
  for (let n = 2; slugs.has(slug); n++) slug = `${slugify(name)}-${n}`;
  slugs.add(slug);
  const aliases = [...new Set((cldr.default ?? []).map(word => dashes(word).toLowerCase()))].filter(word => word !== name.toLowerCase());
  emoji.push({ glyph, name, slug, aliases, group });
}

// Groups with no fully-qualified entries (the component group) carry nothing
// for the palette, so they are dropped.
const used = new Set(emoji.map(entry => entry.group));
const output = {
  groups: groups.filter(entry => used.has(entry.id)),
  emoji,
};
const lines = [
  "{",
  `  "groups": [`,
  output.groups.map(entry => `    ${JSON.stringify(entry)}`).join(",\n"),
  "  ],",
  `  "emoji": [`,
  emoji.map(entry => `    ${JSON.stringify(entry)}`).join(",\n"),
  "  ]",
  "}",
  "",
];
writeFileSync(OUTPUT, lines.join("\n"));
console.log(`wrote ${emoji.length} emoji in ${output.groups.length} groups to ${OUTPUT}`);
