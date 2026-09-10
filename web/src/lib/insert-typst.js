// Typst source builder for the shared Insert menu.  The returned placeholder
// is always an exact substring of text, allowing the editor to put the caret
// in the first useful field after insertion.

const textOf = (value, fallback = "") => value == null ? fallback : String(value);
const numberOf = (value, fallback, minimum = 1, maximum = 100) =>
  Math.min(maximum, Math.max(minimum, Math.floor(Number.isFinite(+value) ? +value : fallback)));
const selected = (context) => textOf(context?.selection?.text || "");
const markup = (value, fallback = "") => textOf(value, fallback).replace(/[\\#\[\]{}$*_<>@]/g, "\\$&");
const string = (value, fallback = "") => JSON.stringify(textOf(value, fallback)).slice(1, -1);
const identifier = (value, fallback = "label") => textOf(value, fallback).trim()
  .replace(/[^\p{L}\p{N}:._-]+/gu, "-").replace(/^-+|-+$/g, "") || fallback;

function finish(text, placeholder = "", notes = [], additionalEdits = []) {
  const result = { text, notes };
  if (placeholder && text.includes(placeholder)) result.placeholder = placeholder;
  if (additionalEdits.length) result.additionalEdits = additionalEdits;
  return result;
}

function table(options) {
  const rows = numberOf(options.rows, 3, 1, 100);
  const columns = numberOf(options.columns, 3, 1, 30);
  const header = options.header !== false;
  const cells = [];
  for (let row = 0; row < rows; row += 1) {
    for (let column = 0; column < columns; column += 1) {
      const value = row === 0 && header ? `Header ${column + 1}` : `Cell ${row + 1},${column + 1}`;
      cells.push(`[${markup(value)}]`);
    }
  }
  const alignment = ["left", "center", "right"].includes(options.alignment) ? `, align: ${options.alignment}` : "";
  const body = `#table(columns: ${columns}${alignment},\n  ${header ? "table.header(" + cells.splice(0, columns).join(", ") + "),\n  " : ""}${cells.join(",\n  ")}\n)`;
  if (!options.caption && !options.label) return finish(body, header ? "Header 1" : "Cell 1,1");
  const caption = markup(options.caption || "Table");
  const label = identifier(options.label, "table");
  return finish(`#figure(${body.slice(1)}, caption: [${caption}]) <${label}>`, header ? "Header 1" : "Cell 1,1");
}

function math(id, options, context) {
  let body = selected(context) || (id === "aligned-math" ? "x &= y \\\nz &= w" : id === "gather-math" ? "x = y \\\nz = w" : "x = y");
  if (id === "inline-math") return finish('$' + body + '$', body);
  if (id === "matrix") {
    const rows = numberOf(options.rows, 3), columns = numberOf(options.columns, 3);
    const delim = ({ brackets: '"["', braces: '"{"', bars: '"|"', doublebars: '"‖"', none: 'none' })[options.brackets] || '"("';
    body = 'mat(delim: ' + delim + ', ' + Array.from({length: rows}, () => Array(columns).fill('0').join(', ')).join('; ') + ')';
  }
  if (id === "cases") body = 'cases(' + Array.from({length: numberOf(options.rows, 2)}, (_,i) => (i+1) + ' & "if" x ' + (i ? '>' : '<=') + ' 0').join(', ') + ')';
  const formula = options.inMath ? body : options.numbered === true
    ? '#math.equation(block: true, numbering: "(1)")[$ ' + body + ' $]'
    : '$ ' + body + ' $';
  return finish(formula + (options.label ? ' <' + identifier(options.label) + '>' : ''), id === 'matrix' ? '0' : body);
}

export function buildTypst(id, options = {}, context = {}) {
  const choice = selected(context);
  const title = markup(options.title || choice || "Heading");
  const label = identifier(options.label, "label");
  const content = choice || "Content.";
  switch (id) {
    case "heading": {
      const level = numberOf(options.level, 1, 1, 6);
      const result = options.numbered === false
        ? `#heading(level: ${level}, numbering: none)[${title}]`
        : `${"=".repeat(level)} ${title}`;
      return finish((options.numbered === true ? `#heading(level: ${level}, numbering: "1.1")[${title}]` : result) + (options.label ? ` <${label}>` : ""), title);
    }
    case "abstract": return finish(`#block[\n*Abstract.* ${choice || "Abstract text."}\n]`, choice || "Abstract text.");
    case "appendix": return finish(`#counter(heading).update(0)
#set heading(numbering: "A.1")
= Appendix`, "Appendix");
    case "toc": return finish("#outline()", "#outline()");
    case "figure": {
      if (!options.src) throw Error("Choose an image first.");
      const source = string(options.src);
      const width = options.width ? `, width: ${string(options.width)}` : "";
      const caption = markup(options.caption, "Figure");
      return finish(options.caption || options.label ? `#figure(image("${source}"${width})${options.caption ? `, caption: [${caption}]` : ""})${options.label ? ` <${label}>` : ""}` : `#image("${source}"${width})`, caption);
    }
    case "table": return table(options);
    case "citation": {
      const keys = Array.isArray(options.keys) ? options.keys.filter(Boolean) : [];
      if (!keys.length) throw Error("Choose at least one bibliography entry.");
      const locator = textOf(options.locator).trim();
      for (const key of keys) if (!/^[\p{L}\p{N}_.:+-]+$/u.test(key)) throw Error("Unsupported citation key.");
      const form = options.style === "narrative" ? ', form: "prose"' : options.style === "parenthetical" ? ', form: "normal"' : '';
      return finish(keys.map(key => '#cite(<' + key + '>' + form + (locator ? ', supplement: [' + markup(locator) + ']' : '') + ')').join(' '));
    }
    case "bibliography": {
      if (!options.file) throw Error("Choose a bibliography file.");
      return finish('#bibliography("' + string(options.file) + '")');
    }
    case "cross-reference": {
      const target=identifier(options.target, "label");
      return finish('#context { let target = query(<'+target+'>).first(); if target.has("numbering") and target.numbering != none { ref(<'+target+'>) } else { link(<'+target+'>)['+markup(target)+'] } }');
    }
    case "label": return finish(`<${label}>`, label);
    case "inline-math":
    case "display-math":
    case "aligned-math":
    case "gather-math":
    case "cases":
    case "matrix": return math(id, options, context);
    case "bulleted-list": return finish((choice ? choice.split(/\r?\n/).filter(Boolean) : ["First item", "Second item"]).map((item) => `- ${item}`).join("\n"), choice || "First item");
    case "numbered-list": return finish((choice ? choice.split(/\r?\n/).filter(Boolean) : ["First item", "Second item"]).map((item) => `+ ${item}`).join("\n"), choice || "First item");
    case "description-list": return finish((choice ? choice.split(/\r?\n/).filter(Boolean) : ["Term: Description"]).map((item) => { const [term, ...rest] = item.split(/:\s*/); return `/ ${markup(term, "Term")}: ${rest.join(": ") || "Description"}`; }).join("\n"), choice || "Term");
    case "quote":
    case "quotation": return finish(`#quote(block: true)[${content}]`, choice || "Content.");
    case "code-block": {
      const language = string(options.language);
      const code = string(choice || "code");
      return finish(`#raw("${code}", block: true, lang: "${language}")`, code);
    }
    case "footnote": return finish(`#footnote[${content}]`, choice || "Content.");
    case "link": return finish(`#link("${string(options.url, "https://example.com")}")[${(choice || markup(options.title || "Link"))}]`, markup(options.text, choice || "Link"));
    case "theorem":
    case "lemma":
    case "proposition":
    case "definition":
    case "proof":
    case "example":
    case "remark": {
      const name = id[0].toUpperCase() + id.slice(1);
      const heading = options.title ? ` *${markup(options.title)}.*` : "";
      const anchor = options.label ? ` <${label}>` : "";
      const numbering = options.numbered === false ? ", numbering: none" : ', numbering: "1"';
      return finish(`#figure([*${name}.*${heading} ${content}], kind: "${name.toLowerCase()}", supplement: [${name}]${numbering})${anchor}`, choice || "Content.");
    }
    case "page-break": return finish("#pagebreak()", "#pagebreak()");
    case "horizontal-rule": return finish("#line(length: 100%)", "#line(length: 100%)");
    case "columns": {
      const count = numberOf(options.count ?? options.columns, 2, 2, 6);
      const gap = options.gap ? `, gutter: ${string(options.gap)}` : "";
      return finish(`#columns(${count}${gap})[${content}]`, choice || "Content.");
    }
    case "custom-environment": {
      const name = textOf(options.environment).trim();
      if (!/^[A-Za-z][\w-]*$/.test(name)) throw Error("Choose a defined content function.");
      const source = [context.text, context.mainText, ...(context.files || []).map(file => file.text)].filter(Boolean).join('\n');
      if (!new RegExp('#let\\s+' + name + '\\s*\\([^)]*\\bbody\\b[^)]*\\)').test(source)) throw Error("Define this content function before inserting it.");
      const signature=source.match(new RegExp('#let\\s+'+name+'\\s*\\(([^)]*)\\)'));
      let index=0;
      const args=signature[1].split(',').map(part=>part.trim()).filter(part=>part&&!part.includes(':')).map(part=>{
        if(part==='body')return '['+content+']';
        const value=options.arguments?.[index++];
        if(!value)throw Error('Enter the '+part+' argument for '+name+'.');
        return value;
      });
      return finish('#'+name+'('+args.join(', ')+')',choice||'Content.');
    }
    default: return finish("", "", ["Unsupported Typst insertion."]);
  }
}
