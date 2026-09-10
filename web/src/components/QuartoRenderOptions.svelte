<script>
  import { parseRenderOptions } from "../lib/quarto-options.js";

  let {
    options = { format: "default", profile: "", parameters: {} },
    disabled = false,
    onapply,
    error: externalError = "",
  } = $props();

  const FORMATS = [
    ["default", "Document (default)"],
    ["html", "HTML"],
    ["pdf", "PDF"],
    ["docx", "Word (DOCX)"],
    ["revealjs", "Reveal.js"],
  ];
  const FORMAT_LABELS = Object.fromEntries(FORMATS);

  let draftFormat = $state("default");
  let draftProfile = $state("");
  let parametersText = $state("{}");
  let validationError = $state("");
  let applyError = $state("");
  let applying = $state(false);
  let dirty = $state(false);
  let observedOptions = null;
  const appliedOptions = $derived(normalOptions(options));

  function normalOptions(value) {
    const source = value && typeof value === "object" ? value : {};
    const format = FORMATS.some(([key]) => key === source.format) ? source.format : "default";
    const profile = typeof source.profile === "string" ? source.profile : "";
    const parameters = source.parameters && typeof source.parameters === "object" && !Array.isArray(source.parameters)
      ? source.parameters
      : {};
    return { format, profile, parameters };
  }

  function optionsText(value) {
    const parameters = normalOptions(value).parameters;
    try {
      return JSON.stringify(parameters, null, 2) || "{}";
    } catch {
      return "{}";
    }
  }

  function syncOptions(value) {
    const next = normalOptions(value);
    draftFormat = next.format;
    draftProfile = next.profile;
    parametersText = optionsText(next);
    validationError = "";
    applyError = "";
    dirty = false;
  }

  // Sync a newly loaded/rendered selection while the user is still looking at
  // a pristine draft. Once they start typing, a reload must not erase it.
  $effect(() => {
    const source = options;
    if (source === observedOptions) return;
    observedOptions = source;
    if (!dirty) syncOptions(source);
  });

  function parseDraft() {
    try {
      const parsed = parseRenderOptions({
        format: draftFormat,
        profile: draftProfile,
        parameters: parametersText,
      });
      validationError = "";
      return parsed;
    } catch (error) {
      validationError = error?.message || "Render options are invalid.";
      return null;
    }
  }

  function changed() {
    dirty = true;
    applyError = "";
    parseDraft();
  }

  function setFormat(event) {
    draftFormat = event.currentTarget.value;
    changed();
  }

  function setProfile(event) {
    draftProfile = event.currentTarget.value;
    changed();
  }

  function setParameters(event) {
    parametersText = event.currentTarget.value;
    changed();
  }

  async function apply() {
    if (disabled || applying) return;
    const next = parseDraft();
    if (!next) return;

    applying = true;
    applyError = "";
    try {
      await onapply?.(next);
      syncOptions(next);
    } catch (error) {
      applyError = error?.message || "Could not apply render options.";
    } finally {
      applying = false;
    }
  }

  const effectiveDisabled = $derived(disabled || applying);
  const summaryFormat = $derived(FORMAT_LABELS[appliedOptions.format] || "Document (default)");
  const summaryProfile = $derived(appliedOptions.profile);
  const summaryParameterCount = $derived(parameterCount(appliedOptions));
  const summary = $derived([
    "Render options",
    summaryFormat,
    summaryProfile,
    `${summaryParameterCount} parameter${summaryParameterCount === 1 ? "" : "s"}`,
  ].filter(Boolean).join(" · "));

  function parameterCount(value) {
    const parameters = value?.parameters;
    return parameters && typeof parameters === "object" && !Array.isArray(parameters)
      ? Object.keys(parameters).length
      : 0;
  }
</script>

<details class="render-options">
  <summary>{summary}</summary>
  <div class="render-options-body">
    <label class="render-options-field">
      <span class="render-options-label">Format</span>
      <select class="select" aria-label="Render format" value={draftFormat}
              onchange={setFormat} disabled={effectiveDisabled}>
        {#each FORMATS as [value, label]}
          <option {value}>{label}</option>
        {/each}
      </select>
    </label>

    <label class="render-options-field">
      <span class="render-options-label">Profile <span class="panel-meta">(optional)</span></span>
      <input class="input" type="text" value={draftProfile} placeholder="Profile name"
             aria-label="Quarto profile (optional)" autocomplete="off"
             oninput={setProfile} disabled={effectiveDisabled} />
    </label>

    <label class="render-options-field render-options-parameters">
      <span class="render-options-label">Parameters (JSON)</span>
      <textarea class="textarea" rows="4" value={parametersText}
                aria-label="Quarto parameters as JSON"
                aria-describedby="render-options-parameters-help"
                aria-invalid={Boolean(validationError)}
                oninput={setParameters} disabled={effectiveDisabled}></textarea>
    </label>
    <p id="render-options-parameters-help" class="panel-meta">
      Use typed JSON values such as <code>{'{"draft": true, "count": 2, "title": "Example", "missing": null}'}</code>.
    </p>

    {#if validationError}
      <p class="render-options-error" role="alert">{validationError}</p>
    {:else if externalError || applyError}
      <p class="render-options-error" role="alert">{externalError || applyError}</p>
    {/if}

    <div class="render-options-actions">
      <button class="btn preset-filled-primary-500" type="button"
              onclick={() => void apply()}
              disabled={effectiveDisabled || Boolean(validationError)}>
        {applying ? "Applying…" : "Apply options"}
      </button>
    </div>
    <p class="panel-meta render-options-notice">
      Profiles and parameters are local to this browser and document.
    </p>
  </div>
</details>

<style>
  .render-options {
    font-size: var(--panel-font-size);
    line-height: var(--panel-line-height);
  }
  .render-options summary {
    cursor: pointer;
    padding-block: calc(var(--spacing) * 2);
    color: var(--color-surface-950-50);
    font-weight: 600;
  }
  .render-options summary:focus-visible {
    outline: 2px solid var(--color-primary-500);
    outline-offset: 2px;
  }
  .render-options-body {
    display: flex;
    flex-direction: column;
    gap: calc(var(--spacing) * 2);
    padding-bottom: calc(var(--spacing) * 2);
  }
  .render-options-field {
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 2);
  }
  .render-options-label {
    min-width: calc(var(--spacing) * 24);
  }
  .render-options-field .select,
  .render-options-field .input {
    min-width: 0;
    flex: 1;
  }
  .render-options-parameters {
    align-items: flex-start;
  }
  .render-options-parameters .textarea {
    min-width: 0;
    flex: 1;
    resize: vertical;
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: 0.875em;
  }
  .render-options-error {
    margin: 0;
    color: var(--color-error-600-400);
  }
  .render-options-actions {
    display: flex;
    justify-content: flex-start;
  }
  .render-options-notice {
    margin: 0;
  }
  @media (max-width: 34rem) {
    .render-options-field {
      align-items: stretch;
      flex-direction: column;
      gap: var(--spacing);
    }
    .render-options-label {
      min-width: 0;
    }
  }
</style>
