// A command line as typed in a terminal, to and from its argument list.
//
// Quotes group, and a backslash escapes whitespace or a quote. Every other
// backslash is kept, so Windows paths such as C:\Users\me\agent.exe and
// \\server\share\agent.exe mean what they say.

const ESCAPABLE = /[\s"']/;

// Single quotes keep everything literally; a single quote inside becomes '\''.
export function joinArgs(args) {
  if (!Array.isArray(args)) return "";
  return args.map((arg) => !arg || ESCAPABLE.test(arg) || arg.endsWith("\\") ? `'${arg.replaceAll("'", "'\\''")}'` : arg).join(" ");
}

export function splitArgs(text) {
  const args = [];
  let current = "";
  let started = false;
  let quote = "";
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    const next = text[i + 1];
    if (quote === "'") {
      if (ch === "'") quote = "";
      else current += ch;
    } else if (ch === "\\" && next !== undefined && (quote === '"' ? next === '"' : ESCAPABLE.test(next))) {
      current += next;
      started = true;
      i++;
    } else if (quote === '"') {
      if (ch === '"') quote = "";
      else current += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      started = true;
    } else if (/\s/.test(ch)) {
      if (started) args.push(current);
      current = "";
      started = false;
    } else {
      current += ch;
      started = true;
    }
  }
  if (started) args.push(current);
  return args;
}
