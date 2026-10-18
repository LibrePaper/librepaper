// Symbol data for the symbol and math palettes. Each entry is one tile: the
// glyph is what the tile shows, and the latex and typst spellings are what
// an insertion writes. Emoji come from a generated file (see loadEmoji).
import { insertSyntaxContext } from "../insert.js";
import { latexPackageEdits } from "../insert-latex.js";

export const CATEGORIES = [
  { id: "greek", label: "Greek" }, { id: "operators", label: "Operators" }, { id: "relations", label: "Relations" },
  { id: "arrows", label: "Arrows" }, { id: "logic", label: "Logic and sets" }, { id: "accents", label: "Accents" },
  { id: "delimiters", label: "Delimiters" }, { id: "text", label: "Text" }, { id: "emoji", label: "Emoji" },
];

const sym = (category, id, glyph, latex, typst, extra = {}) => ({
  id,
  glyph,
  name: extra.name ?? id,
  aliases: extra.aliases ?? [],
  category,
  kind: extra.kind ?? "symbol",
  latex,
  typst,
  ...(extra.html ? { html: extra.html } : {}),
  ...(extra.package ? { package: extra.package } : {}),
  ...(extra.pair ? { pair: extra.pair } : {}),
  ...(extra.bracket ? { bracket: extra.bracket } : {}),
  ...(extra.glyphs ? { glyphs: extra.glyphs } : {}),
  ...(extra.preview ? { preview: extra.preview } : {}),
});
const operator = (id, glyph, latex, typst, extra) => sym("operators", id, glyph, latex, typst, extra);
const relation = (id, glyph, latex, typst, extra) => sym("relations", id, glyph, latex, typst, extra);
const arrow = (id, glyph, latex, typst, extra) => sym("arrows", id, glyph, latex, typst, extra);
const logic = (id, glyph, latex, typst, extra) => sym("logic", id, glyph, latex, typst, extra);
const accent = (id, glyph, latex, typst, extra = {}) => sym("accents", id, glyph, latex, typst, { ...extra, kind: "accent" });
const delimiter = (id, glyph, latex, typst, extra = {}) => sym("delimiters", id, glyph, latex, typst, { ...extra, kind: "delimiter" });

// Text entries are named for their HTML or TeX names. Uppercase names get an
// "upper-" id, because ids are lowercase and must stay unique.
const text = (name, glyph, latex, extra = {}) => {
  const slug = name.replace(/\s+/g, "-"), id = slug === slug.toLowerCase() ? slug : `upper-${slug.toLowerCase()}`;
  return sym("text", `text-${id}`, glyph, latex, glyph, { ...extra, name, kind: "text" });
};

// Epsilon and phi carry their own spellings: their plain commands draw
// the glyphs the variant rows use, so a tile means one letter in every format.
/** @type {Array<[string, string] | [string, string, { typst?: string, html?: string, aliases?: string[] }]>} */
const GREEK_LOWER = [
  ["alpha", "α"], ["beta", "β"], ["gamma", "γ"], ["delta", "δ"],
  ["epsilon", "ϵ", { typst: "epsilon.alt", html: "&epsiv;", aliases: ["eps", "lunate epsilon"] }], ["zeta", "ζ"],
  ["eta", "η"], ["theta", "θ"], ["iota", "ι"], ["kappa", "κ"], ["lambda", "λ"], ["mu", "μ"],
  ["nu", "ν"], ["xi", "ξ"], ["omicron", "ο"], ["pi", "π"], ["rho", "ρ"], ["sigma", "σ"],
  ["tau", "τ"], ["upsilon", "υ"], ["phi", "ϕ", { typst: "phi.alt", html: "&phiv;", aliases: ["straight phi"] }],
  ["chi", "χ"], ["psi", "ψ"], ["omega", "ω"],
];
const GREEK_VARIANTS = [
  ["varepsilon", "ε", "epsilon", "&epsilon;", ["epsilon", "eps", "curly epsilon"]],
  ["vartheta", "ϑ", "theta.alt", "&thetasym;", ["theta"]],
  ["varpi", "ϖ", "pi.alt", "&piv;", ["pi"]],
  ["varrho", "ϱ", "rho.alt", "&rhov;", ["rho"]],
  ["varsigma", "ς", "sigma.alt", "&sigmaf;", ["sigma"]],
  ["varphi", "φ", "phi", "&phi;", ["phi", "curly phi"]],
];
const GREEK_UPPER = [
  ["Gamma", "Γ"], ["Delta", "Δ"], ["Theta", "Θ"], ["Lambda", "Λ"], ["Xi", "Ξ"], ["Pi", "Π"],
  ["Sigma", "Σ"], ["Upsilon", "Υ"], ["Phi", "Φ"], ["Psi", "Ψ"], ["Omega", "Ω"],
];

// Letters with HTML names: [name, glyph, latex, html name or null].
const LETTERS = [
  ["agrave", "à", "\\`a", "agrave"], ["aacute", "á", "\\'a", "aacute"], ["acirc", "â", "\\^a", "acirc"],
  ["atilde", "ã", "\\~a", "atilde"], ["auml", "ä", "\\\"a", "auml"], ["aring", "å", "\\r{a}", "aring"],
  ["aelig", "æ", "\\ae", "aelig"], ["ccedil", "ç", "\\c{c}", "ccedil"], ["egrave", "è", "\\`e", "egrave"],
  ["eacute", "é", "\\'e", "eacute"], ["ecirc", "ê", "\\^e", "ecirc"], ["euml", "ë", "\\\"e", "euml"],
  ["igrave", "ì", "\\`\\i", "igrave"], ["iacute", "í", "\\'\\i", "iacute"], ["icirc", "î", "\\^\\i", "icirc"],
  ["iuml", "ï", "\\\"\\i", "iuml"], ["ntilde", "ñ", "\\~n", "ntilde"], ["ograve", "ò", "\\`o", "ograve"],
  ["oacute", "ó", "\\'o", "oacute"], ["ocirc", "ô", "\\^o", "ocirc"], ["otilde", "õ", "\\~o", "otilde"],
  ["ouml", "ö", "\\\"o", "ouml"], ["oslash", "ø", "\\o", "oslash"], ["ugrave", "ù", "\\`u", "ugrave"],
  ["uacute", "ú", "\\'u", "uacute"], ["ucirc", "û", "\\^u", "ucirc"], ["uuml", "ü", "\\\"u", "uuml"],
  ["yacute", "ý", "\\'y", "yacute"], ["yuml", "ÿ", "\\\"y", "yuml"], ["szlig", "ß", "\\ss", "szlig"],
  ["Agrave", "À", "\\`A", "Agrave"], ["Aacute", "Á", "\\'A", "Aacute"], ["Acirc", "Â", "\\^A", "Acirc"],
  ["Atilde", "Ã", "\\~A", "Atilde"], ["Auml", "Ä", "\\\"A", "Auml"], ["Aring", "Å", "\\AA", "Aring"],
  ["AElig", "Æ", "\\AE", "AElig"], ["Ccedil", "Ç", "\\c{C}", "Ccedil"], ["Egrave", "È", "\\`E", "Egrave"],
  ["Eacute", "É", "\\'E", "Eacute"], ["Ecirc", "Ê", "\\^E", "Ecirc"], ["Euml", "Ë", "\\\"E", "Euml"],
  ["Igrave", "Ì", "\\`I", "Igrave"], ["Iacute", "Í", "\\'I", "Iacute"], ["Icirc", "Î", "\\^I", "Icirc"],
  ["Iuml", "Ï", "\\\"I", "Iuml"], ["Ntilde", "Ñ", "\\~N", "Ntilde"], ["Ograve", "Ò", "\\`O", "Ograve"],
  ["Oacute", "Ó", "\\'O", "Oacute"], ["Ocirc", "Ô", "\\^O", "Ocirc"], ["Otilde", "Õ", "\\~O", "Otilde"],
  ["Ouml", "Ö", "\\\"O", "Ouml"], ["Oslash", "Ø", "\\O", "Oslash"], ["Ugrave", "Ù", "\\`U", "Ugrave"],
  ["Uacute", "Ú", "\\'U", "Uacute"], ["Ucirc", "Û", "\\^U", "Ucirc"], ["Uuml", "Ü", "\\\"U", "Uuml"],
  ["Yacute", "Ý", "\\'Y", "Yacute"], ["scaron", "š", "\\v{s}", "scaron"], ["Scaron", "Š", "\\v{S}", "Scaron"],
  ["zcaron", "ž", "\\v{z}", "zcaron"], ["Zcaron", "Ž", "\\v{Z}", "Zcaron"], ["ccaron", "č", "\\v{c}", null],
  ["lslash", "ł", "\\l", null], ["Lslash", "Ł", "\\L", null],
];

// Punctuation and marks: [name, glyph, latex, html, aliases].
const MARKS = [
  ["endash", "\u2013", "--", "&ndash;", ["dash", "en dash"]], ["emdash", "\u2014", "---", "&mdash;", ["dash", "em dash"]],
  ["ldquo", "\u201C", "``", "&ldquo;", ["open quote", "opening double quote"]], ["rdquo", "\u201D", "''", "&rdquo;", ["close quote", "closing double quote"]],
  ["lsquo", "\u2018", "`", "&lsquo;", ["open single quote"]], ["rsquo", "\u2019", "'", "&rsquo;", ["apostrophe", "close single quote"]],
  ["bdquo", "„", "„", "&bdquo;", ["low double quote"]], ["laquo", "«", "«", "&laquo;", ["guillemet"]],
  ["raquo", "»", "»", "&raquo;", ["guillemet"]], ["ellipsis", "…", "\\ldots", "&hellip;", ["dots", "ldots"]],
  ["section", "§", "\\S", "&sect;", ["par", "section sign"]], ["paragraph", "¶", "\\P", "&para;", ["pilcrow", "paragraph sign"]],
  ["dagger", "†", "\\dag", "&dagger;", ["obelus"]], ["double dagger", "‡", "\\ddag", "&Dagger;", ["ddagger"]],
  ["copyright", "©", "\\copyright", "&copy;", ["(c)"]], ["registered", "®", "\\textregistered", "&reg;", ["(r)"]],
  ["trademark", "™", "\\texttrademark", "&trade;", ["tm"]], ["pound", "£", "\\pounds", "&pound;", ["sterling", "currency"]],
  ["euro", "€", "\\texteuro", "&euro;", ["currency"]], ["yen", "¥", "\\textyen", "&yen;", ["currency"]],
  ["cent", "¢", "\\textcent", "&cent;", ["currency"]], ["degree", "°", "\\textdegree", "&deg;", ["celsius", "temperature"]],
  ["bullet", "•", "\\textbullet", "&bull;", ["list"]], ["middot", "·", "\\textperiodcentered", "&middot;", ["interpunct"]],
  ["multiplication sign", "×", "\\texttimes", "&times;", ["times"]], ["permille", "‰", "\\textperthousand", "&permil;", ["per mille"]],
  ["micro sign", "µ", "\\textmu", "&micro;", ["mu"]], ["inverted question mark", "¿", "?`", "&iquest;", ["spanish question"]],
  ["inverted exclamation mark", "¡", "!`", "&iexcl;", ["spanish exclamation"]],
];

const greek = [
  ...GREEK_LOWER.map(([name, glyph, overrides = {}]) => sym("greek", name, glyph, `\\${name}`, overrides.typst ?? name, { html: overrides.html ?? `&${name};`, aliases: overrides.aliases ?? [] })),
  ...GREEK_VARIANTS.map(([name, glyph, typst, html, aliases]) => sym("greek", name, glyph, `\\${name}`, typst, { html, aliases })),
  ...GREEK_UPPER.map(([name, glyph]) => sym("greek", `upper-${name.toLowerCase()}`, glyph, `\\${name}`, name, { name, html: `&${name};`, aliases: [`capital ${name.toLowerCase()}`] })),
];

const operators = [
  operator("plus", "+", "+", "plus", { aliases: ["add", "positive"] }),
  operator("minus", "\u2212", "-", "minus", { aliases: ["subtract", "negative"] }),
  operator("pm", "±", "\\pm", "plus.minus", { aliases: ["plus minus", "plusminus"], html: "&plusmn;" }),
  operator("mp", "∓", "\\mp", "minus.plus", { aliases: ["minus plus", "minusplus"], html: "&mnplus;" }),
  operator("times", "×", "\\times", "times", { aliases: ["multiply", "cross"], html: "&times;" }),
  operator("div", "÷", "\\div", "div", { aliases: ["divide"], html: "&divide;" }),
  operator("cdot", "⋅", "\\cdot", "dot.op", { aliases: ["dot", "center dot"], html: "&sdot;" }),
  operator("ast", "∗", "\\ast", "ast.op", { aliases: ["asterisk", "star operator"], html: "&lowast;" }),
  operator("star", "⋆", "\\star", "star.op", { aliases: ["star operator"] }),
  operator("circ", "∘", "\\circ", "compose", { aliases: ["composition", "small circle"], html: "&compfn;" }),
  operator("bullet", "∙", "\\bullet", "bullet", { aliases: ["dot"] }),
  operator("oplus", "⊕", "\\oplus", "plus.o", { aliases: ["direct sum"], html: "&oplus;" }),
  operator("ominus", "⊖", "\\ominus", "minus.o", { html: "&ominus;" }),
  operator("otimes", "⊗", "\\otimes", "times.o", { aliases: ["tensor"], html: "&otimes;" }),
  operator("oslash", "⊘", "\\oslash", "slash.o", { html: "&osol;" }),
  operator("odot", "⊙", "\\odot", "dot.o", { html: "&odot;" }),
  operator("cap", "∩", "\\cap", "inter", { aliases: ["intersection"], html: "&cap;" }),
  operator("cup", "∪", "\\cup", "union", { aliases: ["union"], html: "&cup;" }),
  operator("vee", "∨", "\\vee", "or", { aliases: ["or", "disjunction"], html: "&vee;" }),
  operator("wedge", "∧", "\\wedge", "and", { aliases: ["and", "conjunction"], html: "&wedge;" }),
  operator("setminus", "∖", "\\setminus", "without", { aliases: ["set difference", "without"] }),
  operator("diamond", "⋄", "\\diamond", "diamond", { html: "&diamond;" }),
  operator("triangle", "△", "\\triangle", "triangle.stroked.t", { aliases: ["up triangle"] }),
  operator("dagger", "†", "\\dagger", "dagger", { aliases: ["obelus"], html: "&dagger;" }),
  operator("ddagger", "‡", "\\ddagger", "dagger.double", { aliases: ["double dagger"], html: "&Dagger;" }),
  operator("sum", "∑", "\\sum", "sum", { aliases: ["summation"], html: "&sum;" }),
  operator("prod", "∏", "\\prod", "product", { aliases: ["product"], html: "&prod;" }),
  operator("coprod", "∐", "\\coprod", "product.co", { aliases: ["coproduct"], html: "&coprod;" }),
  operator("bigcup", "⋃", "\\bigcup", "union.big", { aliases: ["union"], html: "&Union;" }),
  operator("bigcap", "⋂", "\\bigcap", "inter.big", { aliases: ["intersection"], html: "&Intersection;" }),
  operator("bigvee", "⋁", "\\bigvee", "or.big", { aliases: ["big or"] }),
  operator("bigwedge", "⋀", "\\bigwedge", "and.big", { aliases: ["big and"] }),
  operator("bigoplus", "⨁", "\\bigoplus", "plus.o.big", { aliases: ["big direct sum"] }),
  operator("bigotimes", "⨂", "\\bigotimes", "times.o.big", { aliases: ["big tensor"] }),
  operator("bigodot", "⨀", "\\bigodot", "dot.o.big", { aliases: ["big dot"] }),
  operator("int", "∫", "\\int", "integral", { aliases: ["integral"], html: "&int;" }),
  operator("iint", "∬", "\\iint", "integral.double", { package: "amsmath", aliases: ["double integral"], html: "&Int;" }),
  operator("iiint", "∭", "\\iiint", "integral.triple", { package: "amsmath", aliases: ["triple integral"], html: "&tint;" }),
  operator("oint", "∮", "\\oint", "integral.cont", { aliases: ["contour integral"], html: "&conint;" }),
  ...["lim", "limsup", "liminf", "sup", "inf", "max", "min", "arg", "det", "gcd", "deg", "dim", "ker", "hom",
    "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh", "tanh", "coth",
    "log", "ln", "exp"].map(name => operator(name, name, `\\${name}`, name)),
];

const relations = [
  relation("equals", "=", "=", "eq", { aliases: ["eq", "equal"] }),
  relation("neq", "≠", "\\neq", "eq.not", { aliases: ["ne", "not equal"], html: "&ne;" }),
  relation("leq", "≤", "\\leq", "lt.eq", { aliases: ["le", "less than or equal"], html: "&le;" }),
  relation("geq", "≥", "\\geq", "gt.eq", { aliases: ["ge", "greater than or equal"], html: "&ge;" }),
  relation("ll", "≪", "\\ll", "lt.double", { aliases: ["much less"], html: "&Lt;" }),
  relation("gg", "≫", "\\gg", "gt.double", { aliases: ["much greater"], html: "&Gt;" }),
  relation("leqslant", "⩽", "\\leqslant", "lt.eq.slant", { package: "amssymb", aliases: ["slanted less than or equal"] }),
  relation("geqslant", "⩾", "\\geqslant", "gt.eq.slant", { package: "amssymb", aliases: ["slanted greater than or equal"] }),
  relation("prec", "≺", "\\prec", "prec", { aliases: ["precedes"], html: "&pr;" }),
  relation("succ", "≻", "\\succ", "succ", { aliases: ["succeeds"], html: "&sc;" }),
  relation("preceq", "⪯", "\\preceq", "prec.eq", { aliases: ["precedes or equal"], html: "&prcue;" }),
  relation("succeq", "⪰", "\\succeq", "succ.eq", { aliases: ["succeeds or equal"], html: "&sccue;" }),
  relation("sim", "∼", "\\sim", "tilde.op", { aliases: ["similar"], html: "&sim;" }),
  relation("simeq", "≃", "\\simeq", "tilde.eq", { aliases: ["asymptotically equal"], html: "&sime;" }),
  relation("approx", "≈", "\\approx", "approx", { aliases: ["almost equal"], html: "&approx;" }),
  relation("cong", "≅", "\\cong", "tilde.equiv", { aliases: ["congruent", "isomorphic"], html: "&cong;" }),
  relation("equiv", "≡", "\\equiv", "equiv", { aliases: ["congruent", "identical"], html: "&equiv;" }),
  relation("propto", "∝", "\\propto", "prop", { aliases: ["proportional"], html: "&prop;" }),
  relation("parallel", "∥", "\\parallel", "parallel", { aliases: ["parallel to"], html: "&par;" }),
  relation("perp", "⊥", "\\perp", "perp", { aliases: ["perpendicular", "orthogonal"], html: "&perp;" }),
  relation("mid", "∣", "\\mid", "divides", { aliases: ["divides", "such that"], html: "&mid;" }),
  relation("nmid", "∤", "\\nmid", "divides.not", { package: "amssymb", aliases: ["does not divide"], html: "&nmid;" }),
  relation("in", "∈", "\\in", "in", { aliases: ["element of", "member of"], html: "&isin;" }),
  relation("notin", "∉", "\\notin", "in.not", { aliases: ["not element of"], html: "&notin;" }),
  relation("ni", "∋", "\\ni", "in.rev", { aliases: ["contains", "owns"], html: "&ni;" }),
  relation("subset", "⊂", "\\subset", "subset", { aliases: ["proper subset"], html: "&sub;" }),
  relation("supset", "⊃", "\\supset", "supset", { aliases: ["proper superset"], html: "&sup;" }),
  relation("subseteq", "⊆", "\\subseteq", "subset.eq", { aliases: ["subset or equal"], html: "&sube;" }),
  relation("supseteq", "⊇", "\\supseteq", "supset.eq", { aliases: ["superset or equal"], html: "&supe;" }),
  relation("nsubseteq", "⊈", "\\nsubseteq", "subset.eq.not", { package: "amssymb", aliases: ["not subset or equal"], html: "&nsube;" }),
  relation("nsupseteq", "⊉", "\\nsupseteq", "supset.eq.not", { package: "amssymb", aliases: ["not superset or equal"], html: "&nsupe;" }),
  relation("subsetneq", "⊊", "\\subsetneq", "subset.neq", { package: "amssymb", aliases: ["strict subset"], html: "&subne;" }),
  relation("supsetneq", "⊋", "\\supsetneq", "supset.neq", { package: "amssymb", aliases: ["strict superset"], html: "&supne;" }),
  relation("varsubsetneq", "⊊", "\\varsubsetneq", "subset.neq", { package: "amssymb", aliases: ["strict subset", "subset"] }),
  relation("sqsubset", "⊏", "\\sqsubset", "subset.sq", { package: "amssymb", aliases: ["square subset"], html: "&sqsub;" }),
  relation("sqsubseteq", "⊑", "\\sqsubseteq", "subset.eq.sq", { aliases: ["square subset or equal"], html: "&sqsube;" }),
  relation("sqsupseteq", "⊒", "\\sqsupseteq", "supset.eq.sq", { aliases: ["square superset or equal"], html: "&sqsupe;" }),
  relation("doteq", "≐", "\\doteq", "eq.dot", { aliases: ["approaches limit"], html: "&esdot;" }),
  relation("models", "⊨", "\\models", "models", { aliases: ["satisfies"] }),
  relation("vdash", "⊢", "\\vdash", "tack.r", { aliases: ["proves", "turnstile"], html: "&vdash;" }),
  relation("dashv", "⊣", "\\dashv", "tack.l", { aliases: ["turnstile"], html: "&dashv;" }),
  relation("asymp", "≍", "\\asymp", "asymp", { aliases: ["asymptotically equivalent"] }),
  relation("nsim", "≁", "\\nsim", "tilde.not", { package: "amssymb", aliases: ["not similar"], html: "&nsim;" }),
  relation("napprox", "≉", "\\not\\approx", "approx.not", { aliases: ["not approximately equal"], html: "&nap;" }),
  relation("nleq", "≰", "\\nleq", "lt.eq.not", { package: "amssymb", aliases: ["not less than or equal"], html: "&nle;" }),
  relation("ngeq", "≱", "\\ngeq", "gt.eq.not", { package: "amssymb", aliases: ["not greater than or equal"], html: "&nge;" }),
  relation("nless", "≮", "\\nless", "lt.not", { package: "amssymb", aliases: ["not less than"], html: "&nlt;" }),
  relation("ngtr", "≯", "\\ngtr", "gt.not", { package: "amssymb", aliases: ["not greater than"], html: "&ngt;" }),
];

const arrows = [
  arrow("rightarrow", "→", "\\rightarrow", "arrow.r", { aliases: ["to", "maps to", "->", "right arrow"], html: "&rarr;" }),
  arrow("leftarrow", "←", "\\leftarrow", "arrow.l", { aliases: ["gets", "<-", "left arrow"], html: "&larr;" }),
  arrow("leftrightarrow", "↔", "\\leftrightarrow", "arrow.l.r", { aliases: ["<->"], html: "&harr;" }),
  arrow("uparrow", "↑", "\\uparrow", "arrow.t", { aliases: ["up"], html: "&uarr;" }),
  arrow("downarrow", "↓", "\\downarrow", "arrow.b", { aliases: ["down"], html: "&darr;" }),
  arrow("updownarrow", "↕", "\\updownarrow", "arrow.t.b", { html: "&varr;" }),
  arrow("nearrow", "↗", "\\nearrow", "arrow.tr", { aliases: ["up right"], html: "&nearr;" }),
  arrow("searrow", "↘", "\\searrow", "arrow.br", { aliases: ["down right"], html: "&searr;" }),
  arrow("nwarrow", "↖", "\\nwarrow", "arrow.tl", { aliases: ["up left"], html: "&nwarr;" }),
  arrow("swarrow", "↙", "\\swarrow", "arrow.bl", { aliases: ["down left"], html: "&swarr;" }),
  arrow("mapsto", "↦", "\\mapsto", "arrow.r.bar", { aliases: ["maps to"], html: "&map;" }),
  arrow("longrightarrow", "⟶", "\\longrightarrow", "arrow.r.long", { aliases: ["long to"], html: "&xrarr;" }),
  arrow("longleftarrow", "⟵", "\\longleftarrow", "arrow.l.long", { html: "&xlarr;" }),
  arrow("longleftrightarrow", "⟷", "\\longleftrightarrow", "arrow.l.r.long", { html: "&xharr;" }),
  arrow("longmapsto", "⟼", "\\longmapsto", "arrow.r.long.bar", { aliases: ["long maps to"], html: "&xmap;" }),
  arrow("hookrightarrow", "↪", "\\hookrightarrow", "arrow.r.hook", { html: "&hookrightarrow;" }),
  arrow("hookleftarrow", "↩", "\\hookleftarrow", "arrow.l.hook", { html: "&hookleftarrow;" }),
  arrow("twoheadrightarrow", "↠", "\\twoheadrightarrow", "arrow.r.twohead", { package: "amssymb", html: "&twoheadrightarrow;" }),
  arrow("twoheadleftarrow", "↞", "\\twoheadleftarrow", "arrow.l.twohead", { package: "amssymb" }),
  arrow("rightharpoonup", "⇀", "\\rightharpoonup", "harpoon.rt", { aliases: ["harpoon"] }),
  arrow("leftharpoonup", "↼", "\\leftharpoonup", "harpoon.lt", { aliases: ["harpoon"] }),
  arrow("rightharpoondown", "⇁", "\\rightharpoondown", "harpoon.rb", { aliases: ["harpoon"] }),
  arrow("leftharpoondown", "↽", "\\leftharpoondown", "harpoon.lb", { aliases: ["harpoon"] }),
  arrow("leadsto", "⇝", "\\leadsto", "arrow.r.squiggly", { package: "amssymb", aliases: ["squiggly arrow"] }),
  arrow("nrightarrow", "↛", "\\nrightarrow", "arrow.r.not", { package: "amssymb", aliases: ["not to"] }),
  arrow("nleftarrow", "↚", "\\nleftarrow", "arrow.l.not", { package: "amssymb", aliases: ["not gets"] }),
  arrow("nleftrightarrow", "↮", "\\nleftrightarrow", "arrow.l.r.not", { package: "amssymb" }),
  arrow("double-rightarrow", "⇒", "\\Rightarrow", "arrow.double.r", { name: "Rightarrow", aliases: ["implies", "=>", "double arrow right"], html: "&rArr;" }),
  arrow("double-leftarrow", "⇐", "\\Leftarrow", "arrow.double.l", { name: "Leftarrow", aliases: ["implied by", "<=", "double arrow left"], html: "&lArr;" }),
  arrow("double-leftrightarrow", "⇔", "\\Leftrightarrow", "arrow.double.l.r", { name: "Leftrightarrow", aliases: ["iff", "<=>", "if and only if"], html: "&hArr;" }),
  arrow("double-uparrow", "⇑", "\\Uparrow", "arrow.double.t", { name: "Uparrow", html: "&uArr;" }),
  arrow("double-downarrow", "⇓", "\\Downarrow", "arrow.double.b", { name: "Downarrow", html: "&dArr;" }),
  arrow("double-updownarrow", "⇕", "\\Updownarrow", "arrow.double.t.b", { name: "Updownarrow", html: "&vArr;" }),
  arrow("double-longrightarrow", "⟹", "\\Longrightarrow", "arrow.double.r.long", { name: "Longrightarrow", aliases: ["implies"], html: "&xrArr;" }),
  arrow("double-longleftarrow", "⟸", "\\Longleftarrow", "arrow.double.l.long", { name: "Longleftarrow", aliases: ["implied by"], html: "&xlArr;" }),
  arrow("double-longleftrightarrow", "⟺", "\\Longleftrightarrow", "arrow.double.l.r.long", { name: "Longleftrightarrow", aliases: ["iff"], html: "&xhArr;" }),
  arrow("double-nrightarrow", "⇏", "\\nRightarrow", "arrow.double.r.not", { name: "nRightarrow", package: "amssymb", aliases: ["not implies"] }),
];

const logicAndSets = [
  logic("forall", "∀", "\\forall", "forall", { aliases: ["for all", "universal"], html: "&forall;" }),
  logic("exists", "∃", "\\exists", "exists", { aliases: ["there exists"], html: "&exist;" }),
  logic("nexists", "∄", "\\nexists", "exists.not", { package: "amssymb", aliases: ["there does not exist"], html: "&nexist;" }),
  logic("neg", "¬", "\\neg", "not", { aliases: ["lnot", "not", "negation"], html: "&not;" }),
  logic("land", "∧", "\\land", "and", { aliases: ["and", "wedge"], html: "&and;" }),
  logic("lor", "∨", "\\lor", "or", { aliases: ["or", "vee"], html: "&or;" }),
  logic("emptyset", "∅", "\\emptyset", "emptyset", { aliases: ["empty set", "null set"], html: "&empty;" }),
  logic("infty", "∞", "\\infty", "infinity", { aliases: ["infinity", "inf"], html: "&infin;" }),
  logic("aleph", "ℵ", "\\aleph", "aleph", { aliases: ["cardinal"], html: "&aleph;" }),
  logic("beth", "ℶ", "\\beth", "beth", { package: "amssymb", html: "&beth;" }),
  logic("gimel", "ℷ", "\\gimel", "gimel", { package: "amssymb", html: "&gimel;" }),
  logic("daleth", "ℸ", "\\daleth", "daleth", { package: "amssymb", html: "&daleth;" }),
  logic("hbar", "ℏ", "\\hbar", "h.bar", { aliases: ["planck"], html: "&hbar;" }),
  logic("ell", "ℓ", "\\ell", "ell", { aliases: ["script l", "liter"], html: "&ell;" }),
  logic("prime", "′", "\\prime", "prime", { aliases: ["derivative", "apostrophe"] }),
  logic("nabla", "∇", "\\nabla", "nabla", { aliases: ["gradient", "del"], html: "&nabla;" }),
  logic("partial", "∂", "\\partial", "partial", { aliases: ["partial derivative", "d"], html: "&part;" }),
  logic("top", "⊤", "\\top", "top", { aliases: ["transpose", "true"], html: "&top;" }),
  logic("bot", "⊥", "\\bot", "bot", { aliases: ["false", "bottom"], html: "&bot;" }),
  logic("bb-n", "ℕ", "\\mathbb{N}", "NN", { name: "N", package: "amssymb", aliases: ["naturals", "natural numbers", "blackboard n"], html: "&Nopf;" }),
  logic("bb-z", "ℤ", "\\mathbb{Z}", "ZZ", { name: "Z", package: "amssymb", aliases: ["integers", "blackboard z"], html: "&Zopf;" }),
  logic("bb-q", "ℚ", "\\mathbb{Q}", "QQ", { name: "Q", package: "amssymb", aliases: ["rationals", "blackboard q"], html: "&Qopf;" }),
  logic("bb-r", "ℝ", "\\mathbb{R}", "RR", { name: "R", package: "amssymb", aliases: ["reals", "blackboard r"], html: "&Ropf;" }),
  logic("bb-c", "ℂ", "\\mathbb{C}", "CC", { name: "C", package: "amssymb", aliases: ["complex", "blackboard c"], html: "&Copf;" }),
  logic("angle", "∠", "\\angle", "angle", { aliases: ["angle sign"], html: "&ang;" }),
  logic("therefore", "∴", "\\therefore", "therefore", { package: "amssymb", aliases: ["hence"], html: "&there4;" }),
  logic("because", "∵", "\\because", "because", { package: "amssymb", aliases: ["since"], html: "&becaus;" }),
  logic("square", "□", "\\square", "square.stroked", { package: "amssymb", aliases: ["box", "qed"], html: "&squ;" }),
  logic("blacksquare", "■", "\\blacksquare", "square.filled", { package: "amssymb", aliases: ["qed", "end of proof", "tombstone"] }),
  logic("vdash-double", "⊨", "\\vDash", "models", { package: "amssymb", name: "vDash", aliases: ["double turnstile", "models"] }),
  logic("vdash-big", "⊩", "\\Vdash", "forces", { package: "amssymb", name: "Vdash", aliases: ["forces", "double turnstile"] }),
];

const accents = [
  accent("hat", "x̂", "\\hat{#1}", "hat(#1)", { aliases: ["circumflex"] }),
  accent("widehat", "x̂", "\\widehat{#1}", "hat(#1)", { aliases: ["wide hat"] }),
  accent("bar", "x̄", "\\bar{#1}", "macron(#1)", { aliases: ["macron", "mean"] }),
  accent("overline", "x̅", "\\overline{#1}", "overline(#1)", { aliases: ["complex conjugate", "over bar"] }),
  accent("tilde", "x̃", "\\tilde{#1}", "tilde(#1)", { aliases: ["wiggle"] }),
  accent("widetilde", "x̃", "\\widetilde{#1}", "tilde(#1)", { aliases: ["wide tilde"] }),
  accent("vec", "x⃗", "\\vec{#1}", "arrow(#1)", { aliases: ["vector", "arrow"] }),
  accent("overrightarrow", "x⃗", "\\overrightarrow{#1}", "arrow(#1)", { aliases: ["vector", "arrow"] }),
  accent("dot", "ẋ", "\\dot{#1}", "dot(#1)", { aliases: ["derivative", "time derivative"] }),
  accent("ddot", "ẍ", "\\ddot{#1}", "dot.double(#1)", { aliases: ["second derivative"] }),
  accent("acute", "x́", "\\acute{#1}", "acute(#1)", { aliases: ["accent"] }),
  accent("grave", "x̀", "\\grave{#1}", "grave(#1)", { aliases: ["accent"] }),
  accent("breve", "x̆", "\\breve{#1}", "breve(#1)", { aliases: ["accent"] }),
  accent("check", "x̌", "\\check{#1}", "caron(#1)", { aliases: ["caron", "haček"] }),
  accent("underline", "x̲", "\\underline{#1}", "underline(#1)", { aliases: ["under bar"] }),
];

const PAIR = (latex, typst) => ({ latex, typst });
// A paired delimiter tile. Its spellings are the brackets template with the
// body "x", so the picker and the template agree on the syntax.
const bracketPair = (bracket, name, aliases, glyphs, latex, typst, extra = {}) => sym("delimiters", `pair-${bracket}`, glyphs.join(""), latex, typst, { ...extra, name, aliases, kind: "pair", bracket, glyphs, preview: latex });
const delimiters = [
  bracketPair("parentheses", "Parentheses", ["paren", "parens", "round brackets"], ["(", ")"], "\\left( x \\right)", "(x)"),
  bracketPair("square", "Square brackets", ["bracket", "brackets", "square"], ["[", "]"], "\\left[ x \\right]", "[x]"),
  bracketPair("braces", "Braces", ["curly brackets", "curly", "set"], ["{", "}"], "\\left\\{ x \\right\\}", "{x}"),
  bracketPair("angle", "Angle brackets", ["angle", "chevron", "inner product"], ["⟨", "⟩"], "\\left\\langle x \\right\\rangle", "chevron.l x chevron.r"),
  bracketPair("bars", "Absolute value", ["abs", "modulus", "absolute"], ["|", "|"], "\\left\\lvert x \\right\\rvert", "abs(x)", { package: "amsmath" }),
  bracketPair("norm", "Norm", ["double bars", "double bar", "norm"], ["‖", "‖"], "\\left\\lVert x \\right\\rVert", "norm(x)", { package: "amsmath" }),
  bracketPair("floor", "Floor", ["round down"], ["⌊", "⌋"], "\\left\\lfloor x \\right\\rfloor", "floor(x)"),
  bracketPair("ceil", "Ceiling", ["ceil", "round up"], ["⌈", "⌉"], "\\left\\lceil x \\right\\rceil", "ceil(x)"),
  delimiter("lparen", "(", "(", "(", { name: "left parenthesis", aliases: ["paren", "parentheses"], pair: PAIR(["(", ")"], ["(", ")"]) }),
  delimiter("rparen", ")", ")", ")", { name: "right parenthesis", aliases: ["paren", "parentheses"] }),
  delimiter("lbrack", "[", "[", "[", { name: "left bracket", aliases: ["square bracket"], pair: PAIR(["[", "]"], ["[", "]"]) }),
  delimiter("rbrack", "]", "]", "]", { name: "right bracket", aliases: ["square bracket"] }),
  delimiter("lbrace", "{", "\\{", "{", { name: "left brace", aliases: ["curly bracket", "set"], html: "&lbrace;", pair: PAIR(["\\{", "\\}"], ["{", "}"]) }),
  delimiter("rbrace", "}", "\\}", "}", { name: "right brace", aliases: ["curly bracket"], html: "&rbrace;" }),
  delimiter("langle", "⟨", "\\langle", "chevron.l", { name: "left angle bracket", aliases: ["angle", "inner product"], html: "&lang;", pair: PAIR(["\\langle", "\\rangle"], ["chevron.l", "chevron.r"]) }),
  delimiter("rangle", "⟩", "\\rangle", "chevron.r", { name: "right angle bracket", aliases: ["angle"], html: "&rang;" }),
  delimiter("lfloor", "⌊", "\\lfloor", "floor.l", { name: "left floor", aliases: ["floor"], html: "&lfloor;", pair: PAIR(["\\lfloor", "\\rfloor"], ["floor.l", "floor.r"]) }),
  delimiter("rfloor", "⌋", "\\rfloor", "floor.r", { name: "right floor", aliases: ["floor"], html: "&rfloor;" }),
  delimiter("lceil", "⌈", "\\lceil", "ceil.l", { name: "left ceiling", aliases: ["ceiling", "ceil"], html: "&lceil;", pair: PAIR(["\\lceil", "\\rceil"], ["ceil.l", "ceil.r"]) }),
  delimiter("rceil", "⌉", "\\rceil", "ceil.r", { name: "right ceiling", aliases: ["ceiling", "ceil"], html: "&rceil;" }),
  delimiter("lvert", "|", "\\lvert", "bar.v", { name: "left absolute value", package: "amsmath", aliases: ["abs", "norm bars", "absolute value"], pair: PAIR(["\\lvert", "\\rvert"], ["bar.v", "bar.v"]) }),
  delimiter("rvert", "|", "\\rvert", "bar.v", { name: "right absolute value", package: "amsmath", aliases: ["abs", "absolute value"] }),
  delimiter("lvert-double", "‖", "\\lVert", "bar.v.double", { name: "left norm", package: "amsmath", aliases: ["norm", "double bars"], pair: PAIR(["\\lVert", "\\rVert"], ["bar.v.double", "bar.v.double"]) }),
  delimiter("rvert-double", "‖", "\\rVert", "bar.v.double", { name: "right norm", package: "amsmath", aliases: ["norm", "double bars"] }),
  delimiter("vert", "|", "\\vert", "bar.v", { name: "vertical bar", aliases: ["bar", "pipe", "such that"] }),
  delimiter("vert-double", "‖", "\\Vert", "bar.v.double", { name: "double vertical bar", aliases: ["norm", "double bar"] }),
  delimiter("slash", "/", "/", "slash", { name: "slash", aliases: ["divide"] }),
  delimiter("backslash", "\\", "\\backslash", "backslash", { name: "backslash", aliases: ["set minus"] }),
];

const texts = [
  ...MARKS.map(([name, glyph, latex, html, aliases]) => text(name, glyph, latex, { html, aliases })),
  ...LETTERS.map(([name, glyph, latex, html]) => text(name, glyph, latex, html ? { html: `&${html};` } : {})),
];

// The math and symbol entries. Emoji are not listed here; loadEmoji reads them.
export const SYMBOLS = [
  ...greek,
  ...operators,
  ...relations,
  ...arrows,
  ...logicAndSets,
  ...accents,
  ...delimiters,
  ...texts,
];

let emojiPromise = null;

/**
 * The emoji data is generated and imported on first use, so the palette
 * code that does not show emoji never loads the file.
 * @returns {Promise<{groups: {id: string, label: string}[], symbols: object[]}>}
 */
export async function loadEmoji() {
  emojiPromise ??= import("./emoji.json").then(module => {
    const data = module.default ?? module;
    const symbols = data.emoji.map(entry => ({
      id: `emoji-${entry.slug}`,
      glyph: entry.glyph,
      name: entry.name,
      aliases: entry.aliases,
      category: "emoji",
      kind: "symbol",
      group: entry.group,
      latex: `\\emoji{${entry.slug}}`,
      package: "emoji",
      typst: entry.glyph,
      html: entry.glyph,
    }));
    return { groups: data.groups, symbols };
  }).catch(error => {
    // Forget the failed import so the next call tries it again.
    emojiPromise = null;
    throw error;
  });
  return emojiPromise;
}

// Words in names and aliases are split on spaces and these marks.
const WORD_BREAK = /[\s\-_,.&/]+/;
const wordsOf = text => text.split(WORD_BREAK).filter(Boolean);

/**
 * Ranks by how closely an entry matches the query, lowest first: an exact
 * spelling, then a name prefix, an alias or spelling prefix, every query word
 * starting a word of the name or aliases, and a substring anywhere. Entries
 * of the same rank keep their order.
 * @param {string} query
 * @param {object[]} symbols
 * @returns {object[]}
 */
export function searchSymbols(query, symbols) {
  const raw = String(query ?? "").trim();
  const q = raw.toLowerCase().replace(/\s+/g, " ");
  if (!q) return symbols;
  const queryWords = wordsOf(q);
  const rank = symbol => {
    const name = String(symbol.name ?? "").toLowerCase();
    const aliases = (symbol.aliases ?? []).map(alias => alias.toLowerCase());
    const latex = symbol.latex ?? "";
    const typst = symbol.typst ?? "";
    if (symbol.glyph === raw || name === q || aliases.includes(q) || latex === raw || latex === `\\${raw}` || typst === raw) return 0;
    if (name.startsWith(q)) return 1;
    if (aliases.some(alias => alias.startsWith(q))) return 2;
    if ((raw.startsWith("\\") && latex.startsWith(raw)) || (raw.length >= 2 && typst.startsWith(raw))) return 2;
    if (queryWords.length > 0) {
      const words = [...wordsOf(name), ...aliases.flatMap(wordsOf)];
      if (queryWords.every(word => words.some(entry => entry.startsWith(word)))) return 3;
    }
    if (name.includes(q) || aliases.some(alias => alias.includes(q))) return 4;
    return -1;
  };
  return symbols
    .map((symbol, index) => ({ symbol, index, rank: rank(symbol) }))
    .filter(item => item.rank >= 0)
    .sort((a, b) => a.rank - b.rank || a.index - b.index)
    .map(item => item.symbol);
}

/**
 * The bare spelling of a symbol in one format, without any wrapping.
 * @param {object} symbol
 * @param {"latex"|"markdown"|"quarto"|"typst"|"html"} format
 * @param {{inMath?: boolean}} [options]
 * @returns {string}
 */
export function symbolText(symbol, format, { inMath = true } = {}) {
  if (!inMath && symbol.kind === "text") return symbol.glyph;
  if (format === "typst") return symbol.typst;
  if (format === "html") return symbol.html ?? symbol.glyph;
  return symbol.latex;
}

const WRAPPED = new Set(["latex", "markdown", "quarto", "typst"]);

/**
 * Adds a space where a spelling would run into the text beside it. In LaTeX,
 * a control word before a letter runs into it: "\alpha" then "x" reads as
 * "\alphax", an undefined control word. In Typst, "alpha" then "x" reads as
 * one name, "alphax", so a space is needed on either side of a word there too.
 * @param {string} text the spelling about to be inserted
 * @param {string} format
 * @param {string} before the character before the caret, or "" at the start
 * @param {string} after the character after the caret, or "" at the end
 * @returns {string}
 */
export function separated(text, format, before, after) {
  if (format === "latex" || format === "markdown" || format === "quarto") {
    return /\\[A-Za-z]+$/.test(text) && /^[A-Za-z]$/.test(after) ? `${text} ` : text;
  }
  if (format === "typst") {
    let result = text;
    if (/[A-Za-z0-9]$/.test(result) && /^[A-Za-z0-9]$/.test(after)) result += " ";
    if (/^[A-Za-z]/.test(result) && /^[A-Za-z0-9]$/.test(before)) result = ` ${result}`;
    return result;
  }
  return text;
}

/**
 * The spellings to show in a details panel. One line per format comes first,
 * in the order the format reads best, then the code point when the glyph is a
 * single character.
 * @param {object} symbol
 * @param {string} format
 * @returns {{label: string, code: string}[]}
 */
export function symbolSpellings(symbol, format) {
  const glyph = symbol.glyph ?? "";
  const single = [...glyph].length === 1;
  const fill = code => (typeof code === "string" ? code.replaceAll("#1", "x") : code);
  const latex = { label: "LaTeX", code: fill(symbol.latex) };
  const typst = { label: "Typst", code: symbol.kind === "text" ? glyph : fill(symbol.typst) };
  const html = { label: "HTML", code: symbol.html ?? glyph };
  const order = format === "typst" ? [typst, latex] : format === "html" ? [html] : [latex, typst];
  // A Typst line that only repeats a single character is left out, since the Unicode line shows it.
  const lines = order.filter(line => line.code && !(line.label === "Typst" && line.code === glyph && single));
  if (single) lines.push({ label: "Unicode", code: `U+${glyph.codePointAt(0).toString(16).toUpperCase().padStart(4, "0")}` });
  return lines;
}

/**
 * Turns a click on a tile into the text to insert at the caret. Pair and
 * template entries are routed through entryInsertion in catalog.js, which
 * reuses the math templates, so they never reach this function.
 * @param {object} symbol
 * @param {object} context the editor's insert context (see insert.js)
 * @returns {{text: string, additionalEdits: object[], notes: string[], snippet: boolean}}
 */
export function symbolInsertion(symbol, context = {}) {
  const format = context.format;
  const notes = [];
  const selected = context.selection?.text ?? "";
  const inMath = insertSyntaxContext(context) === "math";
  const wrap = body => (!inMath && WRAPPED.has(format) ? `$${body}$` : body);
  const packages = name => (format === "latex" && name ? latexPackageEdits(name, context, notes) : []);
  // The characters beside the caret decide whether the spelling needs a space.
  const source = typeof context.text === "string" ? context.text : "";
  const before = source[context.selection?.from - 1] ?? "";
  const after = source[context.selection?.to] ?? "";
  let text;
  let additionalEdits = [];
  let snippet = false;

  if (symbol.category === "emoji") {
    if (format === "latex") {
      additionalEdits = packages("emoji");
      notes.push("The emoji package needs LuaLaTeX.");
      text = symbol.latex;
    } else {
      text = symbol.glyph;
    }
  } else if (symbol.kind === "text") {
    text = format === "latex" ? symbol.latex : symbol.glyph;
  } else if (symbol.kind === "accent") {
    const spelling = format === "html" ? symbol.glyph : format === "typst" ? symbol.typst : symbol.latex;
    const filled = selected ? spelling.replace("#1", () => selected) : spelling.replace("#1", () => "#{1:x}");
    text = wrap(filled);
    additionalEdits = packages(symbol.package);
    snippet = !selected;
  } else if (symbol.kind === "delimiter" && selected && symbol.pair && format !== "html") {
    const typst = format === "typst";
    const [open, close] = typst ? symbol.pair.typst : symbol.pair.latex;
    const body = typst ? `${open} ${selected} ${close}` : `\\left${open} ${selected} \\right${close}`;
    text = wrap(body);
    additionalEdits = packages(symbol.package);
  } else {
    const spelling = format === "typst" ? symbol.typst : format === "html" ? (symbol.html ?? symbol.glyph) : symbol.latex;
    text = wrap(spelling);
    additionalEdits = packages(symbol.package);
  }

  return { text: separated(text, format, before, after), additionalEdits, notes, snippet };
}
