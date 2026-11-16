// Line arithmetic on a source string. Each takes a position and reads only its own line.

export function lineStartAt(source: string, pos: number): number {
  return pos === 0 ? 0 : source.lastIndexOf('\n', pos - 1) + 1;
}

export function lineEndAt(source: string, pos: number): number {
  const end = source.indexOf('\n', pos);
  return end === -1 ? source.length : end;
}

// True when only white space precedes pos on its line. A block widget cannot start in the middle of
// a line, so a block inside a quote or after a list marker is drawn as plain text.
export function startsLine(source: string, pos: number): boolean {
  return source.slice(lineStartAt(source, pos), pos).trim() === '';
}
