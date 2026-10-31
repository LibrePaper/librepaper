import { m } from "./paraglide/messages.js";

// The colours a project tag can be. The palette is Okabe and Ito's
// colour-blind-safe set, with its gray standing in for black, because black
// vanishes on the dark ground. The keys are what the server stores, so a key
// is never renamed; a name is what a reader sees. The keys must match COLOURS
// in crates/librepaper-engine/src/storage/postgres/tags.rs. Each key is a
// custom property in styles/theme.css. The colours are the same on light and
// dark pages, so a tag keeps its colour across modes.
export const TAG_COLOURS = [
  { key: "gray", name: m.tag_colours_gray() },
  { key: "orange", name: m.tag_colours_orange() },
  { key: "sky-blue", name: m.tag_colours_sky_blue() },
  { key: "bluish-green", name: m.tag_colours_bluish_green() },
  { key: "yellow", name: m.common_yellow() },
  { key: "blue", name: m.common_blue() },
  { key: "vermillion", name: m.tag_colours_vermillion() },
  { key: "reddish-purple", name: m.tag_colours_reddish_purple() },
];

const KNOWN = new Set(TAG_COLOURS.map((colour) => colour.key));

// A colour the palette does not hold reads as gray rather than as nothing, so
// a tag saved by a newer build still has a dot.
export const tagColour = (key) => `var(--tag-${KNOWN.has(key) ? key : "gray"})`;
