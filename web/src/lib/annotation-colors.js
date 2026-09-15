// Annotation colours are document data sent to the renderer, so they remain
// stable values independent of the page theme.
export const HIGHLIGHT_COLORS = ["#f8e16c", "#9ee6b8", "#a9d8ff", "#f5b4d8"];

// And what each of them is called, since a swatch has nothing but its colour
// to go on and a label of "#f8e16c" is read out one character at a time.
const NAMES = {
  "#f8e16c": "Yellow",
  "#9ee6b8": "Green",
  "#a9d8ff": "Blue",
  "#f5b4d8": "Pink",
};

export const colorName = (color) => NAMES[String(color).toLowerCase()] ?? "Custom";
