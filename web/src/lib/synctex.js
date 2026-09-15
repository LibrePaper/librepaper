const SP_PER_BP = 65781.76;
const RECORD = /^[\[\(vhxkg$]?(\d+),(\d+):(-?\d+),(-?\d+)(?::(-?\d+),(-?\d+),(-?\d+))?/;

const cleanPath = (path) => String(path || "").replaceAll("\\", "/").replace(/^\.\//, "");

async function textOf(bytes) {
  if (!bytes) return "";
  const input = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  if (input[0] !== 0x1f || input[1] !== 0x8b) return new TextDecoder().decode(input);
  if (typeof DecompressionStream === "undefined") return "";
  const stream = new Blob([input]).stream().pipeThrough(new DecompressionStream("gzip"));
  return new Response(stream).text();
}

function projectPath(input, paths) {
  const wanted = cleanPath(input);
  const candidates = paths.map(cleanPath);
  return candidates.find((path) => path === wanted)
    || candidates.find((path) => wanted.endsWith(`/${path}`)) || null;
}

/// Parse SyncTeX v1 into the two spatial queries the reader needs. Returned
/// coordinates are PDF big points measured from the top-left of the page.
export async function parse(bytes, projectPaths = []) {
  const source = await textOf(bytes);
  if (!source.startsWith("SyncTeX Version:")) return null;
  const inputs = new Map();
  const records = [];
  let magnification = 1000, unit = 1, xOffset = 0, yOffset = 0, page = 0, content = false;
  for (const line of source.split(/\r?\n/)) {
    if (!content) {
      let match = /^Input:(\d+):(.*)$/.exec(line);
      if (match) inputs.set(Number(match[1]), projectPath(match[2], projectPaths));
      else if ((match = /^Magnification:(\d+)/.exec(line))) magnification = Number(match[1]) || 1000;
      else if ((match = /^Unit:(\d+)/.exec(line))) unit = Number(match[1]) || 1;
      else if ((match = /^X Offset:(-?\d+)/.exec(line))) xOffset = Number(match[1]) || 0;
      else if ((match = /^Y Offset:(-?\d+)/.exec(line))) yOffset = Number(match[1]) || 0;
      else if (line === "Content:") content = true;
      continue;
    }
    const sheet = /^\{(\d+)/.exec(line);
    if (sheet) { page = Number(sheet[1]); continue; }
    if (line.startsWith("Postamble:")) break;
    const match = RECORD.exec(line);
    if (!match || !page) continue;
    const path = inputs.get(Number(match[1]));
    if (!path) continue;
    const scale = unit * (magnification / 1000) / SP_PER_BP;
    const x = (Number(match[3]) + xOffset) * scale;
    const baseline = (Number(match[4]) + yOffset) * scale;
    const width = Math.max(0, Number(match[5] || 0) * scale);
    const height = Math.max(0, Number(match[6] || 0) * scale);
    const depth = Math.max(0, Number(match[7] || 0) * scale);
    records.push({ path, line: Number(match[2]), page, x, y: baseline - height,
      width: Math.max(width, 1), height: Math.max(height + depth, 1) });
  }
  if (!records.length) return null;
  return {
    forward(path, line) {
      const same = records.filter((record) => record.path === cleanPath(path));
      if (!same.length) return null;
      const distance = Math.min(...same.map((record) => Math.abs(record.line - line)));
      const nearest = same.filter((record) => Math.abs(record.line - line) === distance)
        .sort((a, b) => (a.width * a.height) - (b.width * b.height))[0];
      return nearest ? { ...nearest } : null;
    },
    inverse(pageNumber, x, y) {
      let best = null, score = Infinity;
      for (const record of records) {
        if (record.page !== pageNumber) continue;
        const dx = x < record.x ? record.x - x : x > record.x + record.width ? x - record.x - record.width : 0;
        const dy = y < record.y ? record.y - y : y > record.y + record.height ? y - record.y - record.height : 0;
        const next = dx * dx + dy * dy;
        if (next < score || (next === score && best && record.width * record.height < best.width * best.height)) {
          best = record; score = next;
        }
      }
      return best ? { ...best } : null;
    },
  };
}

export function lineAt(text, offset) {
  let line = 1;
  for (let at = 0; at < Math.min(Math.max(0, offset), text.length); at += 1) if (text.charCodeAt(at) === 10) line += 1;
  return line;
}
