// Tables that give meaning to names in the Org tree: entities, math environments, image
// extensions and the keywords that describe the element after them.

// The Unicode character of each entity the grammar recognises.
export const ENTITY_CHARS: Record<string, string> = {
  alpha: 'α', beta: 'β', gamma: 'γ', delta: 'δ', epsilon: 'ϵ', varepsilon: 'ε', zeta: 'ζ', eta: 'η',
  theta: 'θ', vartheta: 'ϑ', iota: 'ι', kappa: 'κ', lambda: 'λ', mu: 'μ', nu: 'ν', xi: 'ξ', pi: 'π',
  varpi: 'ϖ', rho: 'ρ', sigma: 'σ', varsigma: 'ς', tau: 'τ', upsilon: 'υ', phi: 'φ', varphi: 'ϕ',
  chi: 'χ', psi: 'ψ', omega: 'ω',
  Gamma: 'Γ', Delta: 'Δ', Theta: 'Θ', Lambda: 'Λ', Xi: 'Ξ', Pi: 'Π', Sigma: 'Σ', Upsilon: 'Υ',
  Phi: 'Φ', Psi: 'Ψ', Omega: 'Ω',
  to: '→', rightarrow: '→', leftarrow: '←', Rightarrow: '⇒', Leftarrow: '⇐', leftrightarrow: '↔',
  uparrow: '↑', downarrow: '↓',
  times: '×', div: '÷', pm: '±', mp: '∓', cdot: '·', ldots: '…', dots: '…', hellip: '…',
  nbsp: ' ', ndash: '–', mdash: '—', deg: '°',
  ne: '≠', neq: '≠', leq: '≤', geq: '≥', le: '≤', ge: '≥', infty: '∞', sum: '∑', prod: '∏', int: '∫',
  partial: '∂', nabla: '∇', in: '∈', notin: '∉', subset: '⊂', subseteq: '⊆', supset: '⊃', cap: '∩',
  cup: '∪', forall: '∀', exists: '∃', neg: '¬', land: '∧', lor: '∨',
  copy: '©', reg: '®', trade: '™', euro: '€', pound: '£', yen: '¥', cent: '¢', sect: '§', para: '¶',
  dagger: '†', ddagger: '‡', laquo: '«', raquo: '»',
  approx: '≈', equiv: '≡', sim: '∼', propto: '∝', emptyset: '∅', angle: '∠', checkmark: '✓',
  star: '⋆', bullet: '•'
};

// LaTeX environments whose body is math. Other environments stay text.
export const MATH_ENVIRONMENTS = new Set([
  'equation', 'equation*', 'align', 'align*', 'alignat', 'alignat*', 'flalign', 'flalign*',
  'gather', 'gather*', 'multline', 'multline*', 'eqnarray', 'eqnarray*', 'displaymath', 'math'
]);

export const IMAGE_PATH = /\.(?:png|jpe?g|gif|svg|webp|bmp|avif|tiff?)(?:::.*)?$/i;

// Keywords that describe the element right after them.
export const AFFILIATED = /^(?:NAME|CAPTION|HEADER|HEADERS|RESULTS|PLOT|LABEL|ATTR_[A-Z0-9_-]+)$/;

// Keywords that make a run of leading keywords a document header.
export const HEADER_KEYS = new Set(['TITLE', 'SUBTITLE', 'AUTHOR', 'DATE', 'EMAIL']);

export const TODO_KEYS = new Set(['TODO', 'SEQ_TODO', 'TYP_TODO']);

// Blocks drawn as a quotation, and the class each adds.
export const QUOTE_BLOCKS = new Set(['quote', 'verse', 'center']);
