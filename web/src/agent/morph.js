// Update a live DOM subtree in place so it matches a freshly parsed one.
//
// Replacing a preview's body on every compile recreates every image and
// equation: images blink, selections and scroll positions are lost. Here only
// what changed is touched, and unchanged nodes keep their identity.
//
// LaTeXML numbers its ids by position (S1.p3, S1.p3.m1), so adding one
// paragraph renumbers every later one. Ids, source positions and in-page
// links are therefore left out of a node's signature: two paragraphs that
// differ only in numbering are the same paragraph, and get their numbers
// refreshed rather than being recreated.

const VOLATILE = / (?:id|data-sourcepos)="[^"]*"| href="#[^"]*"/g;
const VOLATILE_ATTRIBUTES = ["id", "data-sourcepos", "href"];

function signature(node) {
  if (node.nodeType === 1) return node.outerHTML.replace(VOLATILE, "");
  return `#${node.nodeType}${node.data}`;
}

function setAttribute(element, name, value) {
  // Even an equal write is a mutation to observers; skip it.
  if (element.getAttribute(name) !== value) element.setAttribute(name, value);
}

// Copy only the volatile attributes through two subtrees of the same shape.
function refreshVolatile(old, fresh) {
  if (old.nodeType !== 1 || old.outerHTML === fresh.outerHTML) return;
  for (const name of VOLATILE_ATTRIBUTES) {
    if (fresh.hasAttribute(name)) setAttribute(old, name, fresh.getAttribute(name));
    else if (old.hasAttribute(name)) old.removeAttribute(name);
  }
  for (let i = 0; i < old.children.length; i++) refreshVolatile(old.children[i], fresh.children[i]);
}

// Make `old` become `fresh` (same node type and tag) without replacing it.
function morphNode(old, fresh) {
  if (old.nodeType !== 1) {
    if (old.data !== fresh.data) old.data = fresh.data;
    return;
  }
  for (const { name, value } of [...fresh.attributes]) setAttribute(old, name, value);
  for (const { name } of [...old.attributes]) if (!fresh.hasAttribute(name)) old.removeAttribute(name);
  morphChildren(old, fresh);
}

export function morphChildren(target, source) {
  const old = [...target.childNodes];
  const fresh = [...source.childNodes];
  const oldSig = old.map(signature);
  const freshSig = fresh.map(signature);

  // Unchanged nodes at either end stay where they are.
  let head = 0;
  while (head < old.length && head < fresh.length && oldSig[head] === freshSig[head]) {
    refreshVolatile(old[head], fresh[head]);
    head++;
  }
  let tail = 0;
  while (tail < old.length - head && tail < fresh.length - head
    && oldSig[old.length - 1 - tail] === freshSig[fresh.length - 1 - tail]) {
    refreshVolatile(old[old.length - 1 - tail], fresh[fresh.length - 1 - tail]);
    tail++;
  }

  const oldMiddle = old.slice(head, old.length - tail);
  const freshMiddle = fresh.slice(head, fresh.length - tail);
  const oldMiddleSig = oldSig.slice(head, old.length - tail);

  // Unused old nodes by signature, in document order, so duplicates pair off
  // first to first. `pending` counts the new nodes still waiting for a
  // signature, to keep a morph from stealing a node a later one will match.
  const available = new Map();
  oldMiddle.forEach((node, i) => {
    if (!available.has(oldMiddleSig[i])) available.set(oldMiddleSig[i], []);
    available.get(oldMiddleSig[i]).push(node);
  });
  const pending = new Map();
  for (const sig of freshSig.slice(head, fresh.length - tail)) pending.set(sig, (pending.get(sig) || 0) + 1);

  const used = new Set();
  let next = 0; // first old node not yet consumed, in order
  let place = old[head] ?? null; // the node the next result must precede

  freshMiddle.forEach((node, i) => {
    const sig = freshSig[head + i];
    pending.set(sig, pending.get(sig) - 1);
    let result = available.get(sig)?.shift();
    if (result) {
      refreshVolatile(result, node);
    } else {
      while (next < oldMiddle.length && used.has(oldMiddle[next])) next++;
      const candidate = oldMiddle[next];
      if (candidate && candidate.nodeType === node.nodeType && candidate.nodeName === node.nodeName
        && !pending.get(oldMiddleSig[next])) {
        result = candidate;
        morphNode(result, node);
      } else {
        result = target.ownerDocument.importNode(node, true);
      }
    }
    used.add(result);
    if (result === place) place = place.nextSibling;
    else target.insertBefore(result, place);
  });

  for (const node of oldMiddle) if (!used.has(node)) node.remove();
}
