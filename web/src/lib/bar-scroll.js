// On a phone the top bar steps aside while a page scrolls down and comes back
// on the way up. The reader and the project list share this rule.

/// Whether the bar is hidden after a scroll of `delta` pixels that ended `top` pixels down, with `bottom` pixels still to scroll to the end (negative during overscroll).
export function barHiddenAfter(hidden, delta, top, bottom = Infinity) {
  if (top <= 0) return false;
  // Near the end, hiding the bar makes the page taller, the browser pulls scrollTop back down, and that reads as a scroll up that shows the bar again. Hold still instead.
  if (bottom < 64) return hidden;
  if (Math.abs(delta) < 4) return hidden;
  if (delta > 0 && top > 48) return true;
  if (delta < 0) return false;
  return hidden;
}

/// Calls `onscroll(delta, top, bottom)` for every scroll of an element inside `selector`, from the second scroll of that element on. Returns stop().
export function watchScrolls(selector, onscroll) {
  const scrollTops = new WeakMap();
  function onScroll(event) {
    const target = event.target;
    if (!(target instanceof Element) || !target.closest(selector)) return;
    const scrollTop = target.scrollTop || 0;
    if (!scrollTops.has(target)) {
      scrollTops.set(target, scrollTop);
      return;
    }
    const prev = scrollTops.get(target);
    scrollTops.set(target, scrollTop);
    const bottom = target.scrollHeight - target.clientHeight - scrollTop;
    onscroll(scrollTop - prev, scrollTop, bottom);
  }
  document.addEventListener("scroll", onScroll, { capture: true, passive: true });
  return () => document.removeEventListener("scroll", onScroll, { capture: true });
}

/// Calls `onreveal()` whenever keyboard focus enters the site bar (`body > nav`). Returns stop().
export function revealOnNavFocus(onreveal) {
  function onNavFocus(event) {
    if (event.target.closest("body > nav")) onreveal();
  }
  document.addEventListener("focusin", onNavFocus);
  return () => document.removeEventListener("focusin", onNavFocus);
}
