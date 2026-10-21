// The colours a project tag can be. The keys are what the server stores, so a
// key is never renamed; a name is what a reader sees. Each key is a custom
// property in styles/theme.css with one shade for light pages and one for
// dark, which is why a tag has no free colour picker: the palette is the set
// of colours that were checked against both grounds.
export const TAG_COLOURS = [
  { key: "gray", name: "Gray" },
  { key: "red", name: "Red" },
  { key: "orange", name: "Orange" },
  { key: "yellow", name: "Yellow" },
  { key: "green", name: "Green" },
  { key: "teal", name: "Teal" },
  { key: "blue", name: "Blue" },
  { key: "purple", name: "Purple" },
  { key: "pink", name: "Pink" },
];

const KNOWN = new Set(TAG_COLOURS.map((colour) => colour.key));

// A colour the palette does not hold reads as gray rather than as nothing, so
// a tag saved by a newer build still has a dot.
export const tagColour = (key) => `var(--tag-${KNOWN.has(key) ? key : "gray"})`;
