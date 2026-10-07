// The row of words at the top of the window -- File, Edit, Insert, View,
// Tools -- is one control, not five. That is the whole difference between a
// menu bar and five buttons that happen to sit next to each other: once a
// menu is open, pointing at another word moves there, and the arrow keys
// walk the row. Nothing opens on hover while the bar is at rest.
//
// One bar per window, so the state is the module's rather than a context's:
// a context set by the bar cannot be seen from the snippet the page hands it,
// which is exactly where the menus are written.
//
// On a phone the words are hidden and a menu is opened from the Panels
// trigger instead; it then opens where that trigger's panel was, not under
// its hidden word.
let opened = $state(null);
let anchor = $state(null);

// This is the one box every menu on a phone opens in, hanging from the right
// edge of the Panels trigger; the hamburger panel and the menus chosen from it
// share it so choosing File does not move the panel.
export const PANEL_POSITIONING = {
  placement: "bottom-end",
  gutter: 8,
  flip: true,
  fitViewport: true,
  overflowPadding: 8,
};

// The triggers in the order the bar shows them, read off the document rather
// than registered: menus come and go with what the page can do right now, and
// a list kept by hand would drift from the row people actually see.
function triggers() {
  const bar = typeof document === "undefined" ? null : document.querySelector(".menubar");
  if (!bar) return [];
  const items = /** @type {HTMLButtonElement[]} */ ([...bar.querySelectorAll("[data-menubar]")]);
  return items.filter((item) => !item.disabled && item.dataset.disabled !== "true");
}

export const menubar = {
  get opened() {
    return opened;
  },
  show(id, from = null) {
    opened = id;
    anchor = from;
  },
  // Closing is addressed to a menu, not to the bar: moving from File to Edit
  // opens Edit and then hears File close, and the late word must not undo the
  // early one.
  close(id) {
    if (opened === id) {
      opened = null;
      anchor = null;
    }
  },
  // What makes the row a bar. Only while something is already showing.
  point(id) {
    if (opened !== null && opened !== id) opened = id;
  },
  // Left and right walk the row and wrap, from the trigger or from inside an
  // open panel. These menus have no submenus, so the two keys are free.
  // Walking the row from a menu opened out of the Panels trigger keeps that box.
  step(delta) {
    const items = triggers();
    const here = items.findIndex((item) => item.dataset.menubar === opened);
    if (here < 0 || !items.length) return false;
    opened = items[(here + delta + items.length) % items.length].dataset.menubar;
    return true;
  },
  get anchor() {
    return anchor;
  },
};
