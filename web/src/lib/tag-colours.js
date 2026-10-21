// The colours a project tag can be. The palette is Okabe and Ito's
// colour-blind-safe set, with its gray standing in for black, because black
// vanishes on the dark ground. The keys are what the server stores, so a key
// is never renamed; a name is what a reader sees. The keys must match COLOURS
// in crates/librepaper-engine/src/storage/postgres/tags.rs. Each key is a
// custom property in styles/theme.css. The colours are the same on light and
// dark pages, so a tag keeps its colour across modes.
export const TAG_COLOURS = [
  { key: "gray", name: "Gray" },
  { key: "orange", name: "Orange" },
  { key: "sky-blue", name: "Sky blue" },
  { key: "bluish-green", name: "Bluish green" },
  { key: "yellow", name: "Yellow" },
  { key: "blue", name: "Blue" },
  { key: "vermillion", name: "Vermillion" },
  { key: "reddish-purple", name: "Reddish purple" },
];

const KNOWN = new Set(TAG_COLOURS.map((colour) => colour.key));

// A colour the palette does not hold reads as gray rather than as nothing, so
// a tag saved by a newer build still has a dot.
export const tagColour = (key) => `var(--tag-${KNOWN.has(key) ? key : "gray"})`;
