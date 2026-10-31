// Annotation colours are document data sent to the renderer, so they remain
// stable values independent of the page theme. The four are light tints of
// the Okabe-Ito yellow, bluish green, sky blue and reddish purple.
// Documents store the hex they were highlighted with, so highlights made
// before this palette keep their original colour: nothing maps or validates
// stored colours against this list.
import { m } from "./paraglide/messages.js";

export const HIGHLIGHT_COLORS = ["#f6ee8e", "#80cfb9", "#9ad2f2", "#e0afca"];

// And what each of them is called, since a swatch has nothing but its colour
// to go on and a label of "#f6ee8e" is read out one character at a time.
const NAMES = {
  "#f6ee8e": m.common_yellow(),
  "#80cfb9": m.annotation_colors_green(),
  "#9ad2f2": m.common_blue(),
  "#e0afca": m.annotation_colors_pink(),
};

export const colorName = (color) => NAMES[String(color).toLowerCase()] ?? m.annotation_colors_custom();
