// The context up to `context` units before `from` and after `to`. Each edge
// stops short of a split surrogate pair.
export function passageContext(text, from, to, context = 32) {
  let prefixAt = Math.max(0, from - context);
  const prefixCode = text.charCodeAt(prefixAt);
  if (prefixCode >= 0xdc00 && prefixCode <= 0xdfff) prefixAt++;
  let suffixEnd = Math.min(text.length, to + context);
  const suffixCode = text.charCodeAt(suffixEnd - 1);
  if (suffixEnd > to && suffixCode >= 0xd800 && suffixCode <= 0xdbff) suffixEnd--;
  return {
    prefix: text.slice(prefixAt, from),
    suffix: text.slice(to, suffixEnd),
  };
}

// The passage a source selection names, in the shape the server locates by
// quote. Context stops short of a split surrogate pair at either edge.
export function sourcePassage(text, from, to, path = "", context = 32) {
  if (!(to > from) || !text.slice(from, to).trim()) return null;
  const { prefix, suffix } = passageContext(text, from, to, context);
  return {
    ...(path ? { path } : {}),
    exact: text.slice(from, to),
    prefix,
    suffix,
    position: from,
  };
}
