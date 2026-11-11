// Which items of a tool band fit. Pure: the component measures, this decides.
//
// `items` is the band's items in order, each `{ width, pinned, group }`: its
// measured width, whether it may never move into the overflow menu, and the
// group it is drawn with (a divider stands between neighbouring groups).
// `available` is the room for the items, `gap` the space between neighbours,
// `divider` the width of a group divider, `more` the width of the "..." button.
//
// Items are moved into the menu from the right, skipping pinned ones, until
// the rest, plus the "..." button when anything moved, fits. Returns
// `visible[i]`, `dividers[i]` (a divider stands before item i) and `overflowed`.
export function fitBand({ items, available, gap = 0, divider = 0, more = 0 }) {
  const visible = items.map(() => true);

  function layout() {
    const dividers = items.map(() => false);
    let count = 0;
    let width = 0;
    let previous;
    items.forEach((item, index) => {
      if (!visible[index]) return;
      if (count) width += gap;
      if (count && item.group !== previous) {
        dividers[index] = true;
        width += divider + gap;
      }
      width += item.width;
      count += 1;
      previous = item.group;
    });
    const overflowed = visible.filter((shown) => !shown).length;
    if (overflowed) width += (count ? gap : 0) + more;
    return { dividers, overflowed, width };
  }

  let result = layout();
  while (result.width > available) {
    let last = -1;
    for (let index = items.length - 1; index >= 0; index -= 1) {
      if (visible[index] && !items[index].pinned) {
        last = index;
        break;
      }
    }
    if (last === -1) break;
    visible[last] = false;
    result = layout();
  }
  return { visible, dividers: result.dividers, overflowed: result.overflowed };
}
