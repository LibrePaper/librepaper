<script>
  import { onDestroy, onMount, tick } from "svelte";
  import SymbolSearch from "./SymbolSearch.svelte";
  import { MATH_TEMPLATES, templateInsertion } from "../lib/symbols/templates.js";
  import { SYMBOLS, symbolText, separated } from "../lib/symbols/index.js";
  import { latexPackageEdits } from "../lib/insert-latex.js";
  import { insertSyntaxContext } from "../lib/insert.js";
  import { loadKatex } from "../lib/katex.js";

  /** @typedef {import("../lib/insert.js").InsertContext} InsertContext */
  /** @typedef {{ id: string, label: string, preview: string, slots: Array<{ id: string, label: string, placeholder: string, optional?: boolean }>, latex: (values: any) => string, typst: (values: any) => string, package?: string | ((values: any) => string | null), grid?: boolean }} MathTemplate */

  // The prop is still called onfocus for callers. It is renamed here so the
  // element attribute onfocus on the fields cannot be confused with it.
  let { getContext, oninsert, onfocus: onfocusPanel, oncancel, format = "" } = $props();

  // KaTeX arrives after the picker is on screen. Until then every tile shows
  // its label, so the panel is usable at once and the math fills in later.
  let katex = $state.raw(null);
  /** @type {string | null} */
  let activeId = $state(null);
  /** @type {InsertContext | null} */
  let captured = $state.raw(null);
  /** @type {Record<string, any>} */
  let values = $state({});
  let error = $state("");
  /** @type {HTMLElement | undefined} */
  let root = $state();
  /** @type {HTMLFormElement | undefined} */
  let formEl = $state();
  // The field that last had the caret. A symbol picked from the search goes
  // there, not into whichever field happens to be first.
  /** @type {HTMLInputElement | null} */
  let lastField = null;

  const active = $derived(MATH_TEMPLATES.find((template) => template.id === activeId) ?? null);
  const inMath = $derived(captured ? insertSyntaxContext(captured) === "math" : false);
  /** Numbers and labels exist only in LaTeX, Typst and Quarto. */
  const numberable = $derived(["latex", "typst", "quarto"].includes(captured?.format ?? ""));
  /** @type {number} */
  const rowCount = $derived(Array.isArray(values.rows) ? values.rows.length : Number(values.rows));
  /** The format name, typed as any where the symbol helpers take a string. */
  const capturedFormat = $derived(/** @type {any} */ (captured?.format));
  const isRowTemplate = (/** @type {MathTemplate | null} */ template) => ["aligned", "gathered", "cases"].includes(template?.id ?? "");

  /** Keys and labels of each row in the grid templates, in display order. */
  const rowFields = {
    aligned: [["left", "Left"], ["right", "Right"]],
    gathered: [["value", "Equation"]],
    cases: [["value", "Value"], ["condition", "Condition"]]
  };
  const matrixBrackets = [
    ["parentheses", "Parentheses"], ["square", "Square brackets"], ["braces", "Curly braces"],
    ["bars", "Vertical bars"], ["double-bars", "Double bars"], ["none", "None"]
  ];
  const bracketKinds = [
    ["parentheses", "Parentheses"], ["square", "Square brackets"], ["braces", "Curly braces"],
    ["angle", "Angle brackets"], ["bars", "Vertical bars"], ["norm", "Norm"], ["floor", "Floor"], ["ceil", "Ceiling"]
  ];

  /** @param {MathTemplate} template @param {number} count */
  function emptyRows(template, count) {
    const fields = rowFields[/** @type {keyof typeof rowFields} */ (template.id)] || [];
    return Array.from({ length: count }, () => Object.fromEntries(fields.map(([key]) => [key, ""])));
  }

  /** @param {number} rows @param {number} columns @param {string[][]} previous */
  function gridOf(rows, columns, previous) {
    return Array.from({ length: rows }, (_, r) => Array.from({ length: columns }, (_, c) => previous[r]?.[c] ?? ""));
  }

  /**
   * The state a form starts from. The first slot takes the selection when
   * there is one, because wrapping a selected expression is the usual
   * reason to open a template at all.
   * @param {MathTemplate} template
   * @param {InsertContext} context
   */
  function freshValues(template, context) {
    // Slots first: the grid templates name their own slots rows and cells,
    // and those must be the grid defaults below, not empty strings.
    const next = {};
    for (const slot of template.slots) next[slot.id] = "";
    Object.assign(next, {
      style: "display",
      numbered: false,
      label: "",
      rows: 2,
      columns: 2,
      brackets: "parentheses",
      kind: "parentheses",
      cells: template.id === "matrix" ? gridOf(2, 2, []) : []
    });
    if (isRowTemplate(template)) next.rows = emptyRows(template, 2);
    const selected = context.selection?.text || "";
    if (!template.grid && template.slots.length && selected) next[template.slots[0].id] = selected;
    return next;
  }

  /**
   * The LaTeX preview fills empty slots with their placeholders, so the box
   * has a shape before anything is typed. Optional slots stay empty, because
   * a placeholder there would draw a superscript the author did not ask for.
   * @param {MathTemplate} template
   * @param {Record<string, any>} state
   */
  function previewValues(template, state) {
    const filled = { ...state };
    for (const slot of template.slots) {
      // Grid templates keep their rows and cells in the arrays handled below.
      if (slot.optional || template.grid) continue;
      filled[slot.id] = String(state[slot.id] ?? "").trim() ? state[slot.id] : slot.placeholder;
    }
    if (Array.isArray(state.rows)) {
      filled.rows = state.rows.map((row) => Object.fromEntries(Object.entries(row).map(([key, value]) => [key, String(value).trim() ? value : "x"])));
    }
    if (Array.isArray(state.cells) && state.cells.length) {
      filled.cells = state.cells.map((row) => row.map((cell) => (cell.trim() ? cell : "x")));
    }
    return filled;
  }

  /** @param {string} source */
  function renderMath(source) {
    return katex.renderToString(source, { throwOnError: false, output: "html", displayMode: true });
  }

  const tiles = $derived(MATH_TEMPLATES.map((template) => {
    let html = "";
    if (katex) {
      try { html = renderMath(template.preview); } catch { html = ""; }
    }
    return { template, html };
  }));

  const previewHtml = $derived.by(() => {
    if (!active || !katex) return "";
    try {
      return renderMath(active.latex(previewValues(active, $state.snapshot(values))));
    } catch {
      return "";
    }
  });

  onMount(async () => {
    try {
      katex = await loadKatex();
    } catch {
      // The tiles keep their labels. Math that cannot load is no reason to
      // break the rest of the panel.
    }
  });

  /** @param {unknown} cause @param {string} fallback */
  function messageOf(cause, fallback) {
    return cause && typeof cause === "object" && "message" in cause ? String(cause.message) : fallback;
  }

  /** @param {Event & { currentTarget: HTMLInputElement }} event */
  function rememberField(event) { lastField = event.currentTarget; }

  function firstField() {
    const field = /** @type {HTMLInputElement | null} */ (formEl?.querySelector("[data-math-field]") ?? formEl?.querySelector("input"));
    field?.focus();
  }

  /**
   * Opens a template's form. The context is read once, here, so the
   * insertion lands where the caret was when the author started.
   * @param {string} id
   */
  export async function openTemplate(id) {
    const template = MATH_TEMPLATES.find((item) => item.id === id);
    // The picker says why html has no math forms, and a command must not get
    // past it, nor capture a target on the way.
    if (!template || format === "html") { activeId = null; return; }
    const context = getContext?.() ?? null;
    if (!context) return;
    if (captured) oncancel?.(captured);
    captured = context;
    values = freshValues(template, context);
    error = "";
    lastField = null;
    activeId = id;
    await tick();
    firstField();
  }

  /** Returns to the picker and puts the focus back on the tile that was open. */
  async function back() {
    const id = activeId;
    activeId = null;
    if (captured) oncancel?.(captured);
    captured = null;
    error = "";
    lastField = null;
    await tick();
    const tile = /** @type {HTMLElement | null} */ (root?.querySelector(`[data-template="${id}"]`));
    tile?.focus();
  }

  /** @param {KeyboardEvent} event */
  function onFormKey(event) {
    if (event.key !== "Escape" || event.defaultPrevented) return;
    // The dialog also closes on Escape; this press belongs to the form and
    // goes back to the picker. A press the search already answered is not the
    // form's, so it stays on the form.
    event.preventDefault();
    event.stopPropagation();
    void back();
  }

  /**
   * A symbol picked into a field can need a package the template does not.
   * Any packaged symbol written as a whole command in a value adds its edits.
   * @param {{ additionalEdits: object[], notes: string[] }} result
   * @param {any} snapshot
   * @param {InsertContext} context
   */
  function addPickedPackages(result, snapshot, context) {
    /** @type {string[]} */
    const strings = [];
    const collect = (/** @type {any} */ value) => {
      if (typeof value === "string") strings.push(value);
      else if (Array.isArray(value)) value.forEach(collect);
      else if (value && typeof value === "object") Object.values(value).forEach(collect);
    };
    collect(snapshot);
    const own = typeof active?.package === "function" ? active.package({ ...snapshot }) : active?.package;
    const names = new Set();
    for (const symbol of SYMBOLS) {
      if (!symbol.package || !symbol.latex) continue;
      const escaped = symbol.latex.replace(/[.*+?^$|()[\]{}\\]/g, "\\$&");
      const pattern = new RegExp(`${escaped}(?![A-Za-z])`);
      if (strings.some((text) => pattern.test(text))) names.add(symbol.package);
    }
    for (const name of names) {
      if (name !== own) result.additionalEdits.push(...latexPackageEdits(name, context, result.notes));
    }
  }

  function insert() {
    if (!active || !captured) return;
    let result;
    const snapshot = $state.snapshot(values);
    try { result = templateInsertion(active, snapshot, captured); }
    catch (cause) { error = messageOf(cause, "This math is not valid yet."); return; }
    // Nothing reaches the document when the template wrote no text.
    if (!result.text) { error = result.notes[0] ?? "This math is not valid yet."; return; }
    if (captured.format === "latex") addPickedPackages(result, snapshot, captured);
    try { oninsert?.(result, captured); }
    // The target can move while the form is open. The author keeps their
    // entries and sees why nothing landed.
    catch (cause) { error = messageOf(cause, "The insertion target changed."); return; }
    error = "";
    activeId = null;
    captured = null;
    lastField = null;
    onfocusPanel?.();
  }

  /** @param {number} count */
  function setRowCount(count) {
    if (!active) return;
    const wanted = Math.max(1, count);
    const current = /** @type {any[]} */ (values.rows);
    values.rows = wanted <= current.length
      ? current.slice(0, wanted)
      : [...current, ...emptyRows(active, wanted - current.length)];
  }

  /** @param {number} rows @param {number} columns */
  function setMatrixSize(rows, columns) {
    const r = Math.max(1, rows);
    const c = Math.max(1, columns);
    // Resizing keeps what is still on the grid, so a stepper press does not
    // throw away a matrix someone has typed into.
    values.cells = gridOf(r, c, values.cells);
    values.rows = r;
    values.columns = c;
  }

  /** @param {number} delta */
  function changeRows(delta) {
    if (isRowTemplate(active)) setRowCount(values.rows.length + delta);
    else setMatrixSize(values.rows + delta, values.columns);
  }

  /** @param {number} delta */
  function changeColumns(delta) { setMatrixSize(values.rows, values.columns + delta); }

  /** Inserts a picked symbol at the caret of the last field the author used. */
  function pickSymbol(symbol) {
    const field = lastField?.isConnected ? lastField : /** @type {HTMLInputElement | null} */ (formEl?.querySelector("[data-math-field]"));
    if (!field || !captured) return;
    const start = field.selectionStart ?? field.value.length;
    const end = field.selectionEnd ?? start;
    // Separated first, so a space put before the spelling moves the marker
    // with it and the caret still lands inside the accent.
    let text = separated(symbolText(symbol, capturedFormat, { inMath: true }), capturedFormat, field.value[start - 1] ?? "", field.value[end] ?? "");
    // An accent wraps the selection. With none, the caret goes inside it.
    let inside = -1;
    if (symbol.kind === "accent") {
      if (start < end) text = text.replace("#1", field.value.slice(start, end));
      else {
        inside = text.indexOf("#1");
        text = text.replace("#1", "");
      }
    }
    // Setting the range by hand, then dispatching input, is what lets the
    // bound value see the change and the caret land after the symbol.
    field.setRangeText(text, start, end, "end");
    if (inside >= 0) field.setSelectionRange(start + inside, start + inside);
    field.dispatchEvent(new Event("input", { bubbles: true }));
    lastField = field;
    field.focus();
  }

  // The overlay unmounts the palette when it closes, and a form left open
  // holds an insertion target that the editor must release.
  onDestroy(() => { if (captured) oncancel?.(captured); });

  /** Puts the focus where a keyboard user expects it: a tile, or the first field. */
  export function focus() {
    /** @type {HTMLElement | null} */ (focusTarget())?.focus();
  }

  /** The open form's first field, or the first tile. */
  export function focusTarget() {
    if (activeId) return formEl?.querySelector("[data-math-field]") ?? formEl?.querySelector("input") ?? null;
    return root?.querySelector("[data-template]") ?? null;
  }

  /**
   * Whether an Escape belongs to the palette rather than to the overlay
   * around it. The overlay hears the press first, so the palette acts on it
   * here: a search with text keeps it to clear itself, and an open form goes
   * back to the picker. The overlay then cancels the press, which also keeps
   * onFormKey from going back a second time.
   * @param {KeyboardEvent} event
   */
  export function claimsEscape(event) {
    const target = event.target;
    if (target instanceof HTMLInputElement && target.type === "search" && target.value) return true;
    if (!activeId) return false;
    void back();
    return true;
  }
</script>

{#snippet stepper(name, count, unit, change)}
  <div class="math-stepper" role="group" aria-label={name}>
    <button type="button" class="btn btn-sm" aria-label={`Fewer ${name.toLowerCase()}`} disabled={count <= 1} onclick={() => change(-1)}>-</button>
    <span class="math-count" aria-live="polite">{count} {count === 1 ? unit : `${unit}s`}</span>
    <button type="button" class="btn btn-sm" aria-label={`More ${name.toLowerCase()}`} onclick={() => change(1)}>+</button>
  </div>
{/snippet}

<div class="math-palette" bind:this={root}>
  {#if !active}
    {#if format === "html"}
      <p class="panel-muted">Math templates need LaTeX, Typst, Markdown or Quarto.</p>
    {:else}
      <div class="math-grid">
        {#each tiles as { template, html } (template.id)}
          <!-- The rendered math is the label. The name is a tooltip and what a screen reader says. -->
          <button type="button" class="math-tile" data-template={template.id} aria-label={template.label} title={template.label} onclick={() => openTemplate(template.id)}>
            <span class="math-tile-preview" aria-hidden="true">{#if html}{@html html}{:else}{template.label}{/if}</span>
          </button>
        {/each}
      </div>
    {/if}
  {:else}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="math-form" role="group" aria-label={active.label} onkeydown={onFormKey}>
      <div class="math-form-head">
        <button type="button" class="btn btn-sm" onclick={back}>Back</button>
        <h3 class="math-form-title">{active.label}</h3>
      </div>

      <div class="math-preview" aria-live="polite">
        {#if !katex}
          <span class="panel-muted">Loading preview…</span>
        {:else if previewHtml}
          {@html previewHtml}
        {:else}
          <span class="panel-muted">Nothing to preview yet.</span>
        {/if}
      </div>
      {#if captured?.format === "typst"}
        <p class="panel-muted">Preview uses the LaTeX form.</p>
      {/if}

      <!-- Outside the form: Enter in the search field must not submit it. -->
      <section class="math-symbols" aria-labelledby="math-symbols-heading">
        <h3 class="math-symbols-heading" id="math-symbols-heading">Symbols</h3>
        <SymbolSearch symbols={SYMBOLS} format={capturedFormat} onpick={pickSymbol} placeholder="Search symbols" autofocus={false} />
      </section>

      <form class="math-fields" bind:this={formEl} onsubmit={(event) => { event.preventDefault(); insert(); }}>
        {#if active.id === "brackets"}
          <label class="label">Brackets
            <select class="select" bind:value={values.kind}>
              {#each bracketKinds as [value, label]}<option {value}>{label}</option>{/each}
            </select>
          </label>
        {/if}

        {#if isRowTemplate(active)}
          {@render stepper("Rows", rowCount, "row", changeRows)}
          {#each values.rows as row, index}
            <div class="math-pair" style:grid-template-columns={`repeat(${rowFields[active.id].length}, minmax(0, 1fr))`}>
              {#each rowFields[active.id] as [key, label]}
                <label class="label">{label}
                  <input class="input" data-math-field bind:value={values.rows[index][key]} onfocus={rememberField} />
                </label>
              {/each}
            </div>
          {/each}
        {:else if active.id === "matrix"}
          <div class="math-steppers">
            {@render stepper("Rows", rowCount, "row", changeRows)}
            {@render stepper("Columns", values.columns, "column", changeColumns)}
          </div>
          <label class="label">Brackets
            <select class="select" bind:value={values.brackets}>
              {#each matrixBrackets as [value, label]}<option {value}>{label}</option>{/each}
            </select>
          </label>
          <div class="math-cells" style:grid-template-columns={`repeat(${values.columns}, minmax(0, 1fr))`}>
            {#each values.cells as cellRow, r}
              {#each cellRow as cell, c}
                <input class="input" data-math-field aria-label={`Row ${r + 1}, column ${c + 1}`} bind:value={values.cells[r][c]} onfocus={rememberField} />
              {/each}
            {/each}
          </div>
        {:else}
          {#each active.slots as slot (slot.id)}
            <label class="label">{slot.label}{/** @type {{ optional?: boolean }} */ (slot).optional ? " (optional)" : ""}
              <input class="input" data-math-field placeholder={slot.placeholder} bind:value={values[slot.id]} onfocus={rememberField} />
            </label>
          {/each}
        {/if}

        {#if !inMath}
          <fieldset class="math-style">
            <legend>Style</legend>
            <label><input type="radio" name="math-style" value="display" bind:group={values.style} /> Display</label>
            <label><input type="radio" name="math-style" value="inline" bind:group={values.style} /> Inline</label>
            {#if numberable && values.style === "display"}
              <label><input type="checkbox" name="math-numbered" bind:checked={values.numbered} /> Numbered</label>
            {/if}
          </fieldset>
          {#if numberable && values.style === "display" && values.numbered}
            <label class="label">Label
              <input class="input" name="math-label" placeholder="name" bind:value={values.label} />
            </label>
          {/if}
        {/if}

        {#if error}<p class="math-error" role="alert">{error}</p>{/if}

        <div class="math-actions">
          <button type="button" class="btn" onclick={back}>Cancel</button>
          <button type="submit" class="btn lp-control-tonal-brand">Insert</button>
        </div>
      </form>
    </div>
  {/if}
</div>

<style>
  .math-palette { display: flex; flex-direction: column; gap: var(--panel-section-gap); height: 100%; min-height: 0; overflow-y: auto; overscroll-behavior: contain; }
  /* Equal columns and one aspect ratio, so every tile is the same box whatever the math inside it. */
  .math-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(6rem, 1fr)); gap: .5rem; }
  .math-tile { display: flex; align-items: center; justify-content: center; width: 100%; aspect-ratio: 3 / 2; padding: .5rem .75rem; overflow: hidden; border: 1px solid var(--color-divider); border-radius: var(--radius-base); background: transparent; color: inherit; cursor: pointer; }
  .math-tile:hover, .math-tile:focus-visible { background: var(--color-row-hover); }
  .math-tile-preview { display: flex; align-items: center; justify-content: center; max-width: 100%; font-size: .8rem; }
  .math-form { display: flex; flex-direction: column; gap: .75rem; }
  .math-form-head { display: flex; align-items: center; gap: .5rem; }
  .math-form-title { margin: 0; font: inherit; font-weight: var(--panel-title-weight); }
  .math-preview { min-height: 3.5rem; padding: .6rem; overflow-x: auto; border: 1px solid var(--color-divider); border-radius: var(--radius-base); background: var(--color-subtle); }
  .math-fields { display: flex; flex-direction: column; gap: .6rem; }
  .math-pair { display: grid; gap: .5rem; }
  .math-steppers { display: flex; flex-wrap: wrap; gap: .75rem; }
  .math-stepper { display: flex; align-items: center; gap: .5rem; }
  .math-count { min-width: 5.5rem; color: var(--panel-muted); font-size: var(--panel-meta-size); text-align: center; }
  .math-cells { display: grid; gap: .35rem; }
  .math-style { display: flex; flex-wrap: wrap; gap: 1rem; margin: 0; padding: 0; border: 0; }
  .math-symbols { border-top: 1px solid var(--color-divider); padding-top: .5rem; }
  .math-symbols-heading { font-size: .8rem; font-weight: 600; color: var(--panel-muted); margin-bottom: .35rem; }
  .math-error { padding: .6rem .8rem; color: var(--color-error-text); background: var(--color-raised); border: 1px solid var(--color-error-solid); border-radius: var(--radius-base); }
  .math-actions { display: flex; justify-content: flex-end; gap: .5rem; }
</style>
