// One colour per person, everywhere they appear: the caret in the source, the
// label on it, the badge in the presence strip.
//
// The colour is a function of who they are rather than of when they arrived,
// so two people in the same file are two colours, the same person is the same
// colour in every tab and on every other peer's screen, and somebody who was
// blue yesterday is blue today. Drawing one at random -- which is what this
// replaces -- gave two people the same colour about a third of the time in a
// file with three in it, and changed everybody's colour on every reload.
//
// The six are the ones `styles/librepaper.css` has a `user-color-` rule for,
// picked to stay apart from each other and to carry white text in both themes.
// A hue added here needs its rule there, or the caret falls back to the theme
// colour and two people are the same again.
export const PRESENCE_COLOURS = ["#2f5bd0", "#c2410c", "#15803d", "#7c3aed", "#be123c", "#0e7490"];

/// The colour for `seed` -- a name, a handle, or, for somebody who has not
/// said who they are, their tab's presence id -- avoiding `taken`, the
/// colours the other people in the file are already wearing.
///
/// The seed decides where in the palette to start looking, so somebody alone
/// in a file gets the same colour every time; the walk from there is what
/// keeps two people apart, since six hues and a hash alone put two of any
/// six names together about as often as not. Past six people the palette is
/// exhausted and the seventh shares, which is the point at which the strip
/// shows "+N" rather than a badge anyway.
///
/// An empty seed has no person behind it and gets no colour, so the
/// stylesheet's own fallback applies rather than everyone anonymous sharing
/// the first hue.
export function presenceColour(seed, taken = []) {
  const text = String(seed ?? "").trim();
  if (!text) return "";
  let hash = 0;
  for (const char of text) hash = (hash * 31 + char.codePointAt(0)) >>> 0;
  const start = hash % PRESENCE_COLOURS.length;
  const spoken = new Set(taken);
  for (let step = 0; step < PRESENCE_COLOURS.length; step += 1) {
    const colour = PRESENCE_COLOURS[(start + step) % PRESENCE_COLOURS.length];
    if (!spoken.has(colour)) return colour;
  }
  return PRESENCE_COLOURS[start];
}
