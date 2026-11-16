// The application's one tooltip. Any element can ask for it with attributes,
// whether Svelte markup or DOM built by script:
//
//   data-tip="Save"          the text; absent or empty means no tooltip
//   data-tip-key="Ctrl+S"    an already formatted shortcut, drawn as a keycap
//   data-tip-wait="long"     wait 600 ms instead of 150 ms (list rows, clipped text)
//   data-tip-place="right"   bottom (default), top, right or left
//
// One listener set per document watches for those attributes, and one element
// is drawn and reused, so there is never more than one tip and no component
// has to wrap its trigger. A native title attribute looks different and is
// refused by tests/unit/native-titles.mjs.
const SHORT = 150;
const LONG = 600;
const LEAVE = 100;
// Moving along a row of buttons should not make the tip flicker, so a tip
// that closed this recently is replaced without waiting.
const RECENT = 300;
const GAP = 8;
const MARGIN = 8;
const OPPOSITE = { bottom: "top", top: "bottom", right: "left", left: "right" };

const installed = new WeakSet();

export function installTips(doc = document) {
  if (installed.has(doc)) return;
  installed.add(doc);
  const win = doc.defaultView;
  const supportsPopover = typeof HTMLElement !== "undefined" && "showPopover" in HTMLElement.prototype;

  let tip = null;
  let textNode = null;
  let keyNode = null;
  let current = null; // the trigger the tip is showing for
  let pending = null; // the trigger waiting for its delay
  let openTimer = 0;
  let closeTimer = 0;
  let closedAt = -Infinity;
  let previousDescribedBy = null;
  let observer = null;
  // Whether the last thing the person did was press a pointer. Focus that
  // follows a press is about using the control, not asking what it is.
  let byPointer = false;

  const triggerFor = (target) => {
    if (!(target instanceof win.Element)) return null;
    const trigger = target.closest("[data-tip]");
    return trigger && trigger.getAttribute("data-tip") ? trigger : null;
  };

  // A slow tip is there to spell out text that may be cut off. When it only
  // repeats what its element already shows in full it says nothing, so it is
  // drawn only when some of that text is clipped. A control's tip is always
  // drawn, word beside the icon or not.
  function repeats(trigger) {
    if (trigger.getAttribute("data-tip-wait") !== "long") return false;
    if (trigger.getAttribute("data-tip") !== (trigger.textContent ?? "").trim()) return false;
    for (const node of [trigger, ...trigger.querySelectorAll("*")]) {
      if (node.clientWidth > 0 && (node.scrollWidth > node.clientWidth + 1 || node.scrollHeight > node.clientHeight + 1)) return false;
    }
    return true;
  }

  // Created on first use, so a page that never shows a tip pays nothing.
  function element() {
    if (tip) return tip;
    tip = doc.createElement("div");
    tip.id = "lp-tip";
    tip.setAttribute("role", "tooltip");
    tip.className = "lp-tip card lp-control-neutral px-2 py-1 text-xs shadow-lg";
    if (supportsPopover) tip.setAttribute("popover", "manual");
    else tip.hidden = true;
    textNode = doc.createElement("span");
    textNode.className = "tip-text";
    keyNode = doc.createElement("kbd");
    keyNode.className = "tip-key";
    tip.append(textNode, keyNode);
    doc.body.append(tip);
    return tip;
  }

  function fill(trigger) {
    const text = trigger.getAttribute("data-tip") ?? "";
    const key = trigger.getAttribute("data-tip-key") ?? "";
    // Written only when different, because the observer below watches the
    // tip's own subtree and would otherwise see its own write.
    if (textNode.textContent !== text) textNode.textContent = text;
    if (keyNode.textContent !== key) keyNode.textContent = key;
    keyNode.hidden = !key;
  }

  function place(trigger) {
    const box = trigger.getBoundingClientRect();
    const size = tip.getBoundingClientRect();
    const width = doc.documentElement.clientWidth;
    const height = doc.documentElement.clientHeight;
    const at = (side) => {
      if (side === "top") return { x: box.left + box.width / 2 - size.width / 2, y: box.top - GAP - size.height };
      if (side === "right") return { x: box.right + GAP, y: box.top + box.height / 2 - size.height / 2 };
      if (side === "left") return { x: box.left - GAP - size.width, y: box.top + box.height / 2 - size.height / 2 };
      return { x: box.left + box.width / 2 - size.width / 2, y: box.bottom + GAP };
    };
    const fits = (side) => {
      const { x, y } = at(side);
      if (side === "top") return y >= MARGIN;
      if (side === "left") return x >= MARGIN;
      if (side === "right") return x + size.width <= width - MARGIN;
      return y + size.height <= height - MARGIN;
    };
    let side = trigger.getAttribute("data-tip-place");
    if (!(side in OPPOSITE)) side = "bottom";
    if (!fits(side) && fits(OPPOSITE[side])) side = OPPOSITE[side];
    const { x, y } = at(side);
    tip.style.left = `${Math.max(MARGIN, Math.min(x, width - size.width - MARGIN))}px`;
    tip.style.top = `${Math.max(MARGIN, Math.min(y, height - size.height - MARGIN))}px`;
  }

  // A tip that only repeats the control's own name would be read twice by a
  // screen reader, so it is attached as a description only when it adds to it.
  function describe(trigger) {
    previousDescribedBy = trigger.getAttribute("aria-describedby");
    const text = trigger.getAttribute("data-tip");
    if (text === trigger.getAttribute("aria-label") || text === (trigger.textContent ?? "").trim()) return;
    trigger.setAttribute("aria-describedby", tip.id);
  }

  function undescribe(trigger) {
    if (previousDescribedBy === null) trigger.removeAttribute("aria-describedby");
    else trigger.setAttribute("aria-describedby", previousDescribedBy);
  }

  function show(trigger) {
    clearTimeout(openTimer);
    clearTimeout(closeTimer);
    pending = null;
    if (current === trigger) return;
    if (current) undescribe(current);
    const wasOpen = current !== null;
    current = trigger;
    const el = element();
    fill(trigger);
    describe(trigger);
    if (!wasOpen) {
      if (supportsPopover) el.showPopover();
      else {
        (trigger.closest("dialog") ?? doc.body).append(el);
        el.style.zIndex = "2147483647";
        el.hidden = false;
      }
      observer = new win.MutationObserver(refresh);
      observer.observe(doc.body, { subtree: true, childList: true, attributes: true, attributeFilter: ["data-tip", "data-tip-key"] });
    }
    place(trigger);
  }

  // The trigger changed or went away while its tip was showing.
  function refresh() {
    if (!current) return;
    if (!current.isConnected || !current.getAttribute("data-tip")) return close();
    fill(current);
    place(current);
  }

  // Everything goes: the open tip and the one still waiting.
  function close() {
    clearTimeout(openTimer);
    pending = null;
    hide();
  }

  // Only the open tip goes, so one that is waiting elsewhere still gets its turn.
  function hide() {
    clearTimeout(closeTimer);
    if (!current) return;
    undescribe(current);
    current = null;
    observer?.disconnect();
    observer = null;
    if (supportsPopover) tip.hidePopover();
    else tip.hidden = true;
    closedAt = performance.now();
  }

  function open(trigger) {
    if (trigger === current) return void clearTimeout(closeTimer);
    if (trigger === pending) return;
    clearTimeout(openTimer);
    pending = null;
    if (repeats(trigger)) return;
    // A slow tip always waits, even beside an open one, so resting on one row
    // of a list does not send a tip down the list after the pointer. The open
    // tip is left to close on its own timer meanwhile.
    const long = trigger.getAttribute("data-tip-wait") === "long";
    if (!long && (current !== null || performance.now() - closedAt < RECENT)) return show(trigger);
    pending = trigger;
    openTimer = setTimeout(() => show(trigger), long ? LONG : SHORT);
  }

  doc.addEventListener("pointerover", (event) => {
    if (event.pointerType === "touch") return;
    const trigger = triggerFor(event.target);
    if (trigger) open(trigger);
  });

  doc.addEventListener("pointerout", (event) => {
    const trigger = triggerFor(event.target);
    if (!trigger || (event.relatedTarget instanceof win.Node && trigger.contains(event.relatedTarget))) return;
    if (trigger === pending) {
      clearTimeout(openTimer);
      pending = null;
    } else if (trigger === current) {
      clearTimeout(closeTimer);
      closeTimer = setTimeout(hide, LEAVE);
    }
  });

  doc.addEventListener("focusin", (event) => {
    const trigger = triggerFor(event.target);
    if (trigger && !byPointer) open(trigger);
  });

  doc.addEventListener("focusout", (event) => {
    const trigger = triggerFor(event.target);
    if (trigger && (trigger === current || trigger === pending)) close();
  });

  doc.addEventListener("keydown", (event) => {
    if (event.isTrusted) byPointer = false;
    if (event.key === "Escape") close();
  }, true);
  doc.addEventListener("pointerdown", (event) => {
    if (event.isTrusted) byPointer = true;
    close();
  }, true);
  doc.addEventListener("click", close, true);
  doc.addEventListener("scroll", close, { capture: true, passive: true });
  win.addEventListener("blur", close);
}
