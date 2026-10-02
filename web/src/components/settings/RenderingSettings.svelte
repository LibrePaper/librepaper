<script>
  // Two things Quarto may take when it builds a project here: a profile and
  // parameters. Both are optional, both are kept with this document's build
  // preferences in this browser only, which is where options comes from. A
  // project that declares neither needs nothing here. There is no format
  // choice: the live preview is always the page Quarto renders, in whatever
  // format the document's front matter says.
  import SettingRow from "./SettingRow.svelte";
  import { parseRenderOptions } from "../../lib/quarto-options.js";

  let { options, onapplyoptions, disabled = false, scopeKey = "" } = $props();

  let draftProfile = $state("");
  let parametersText = $state("{}");
  let validationError = $state("");
  let applyError = $state("");
  let applying = $state(false);
  let dirty = $state(false);
  let observedOptions = null;
  let observedScopeKey;
  let applyRequest = 0;

  function parametersOf(value) {
    const parameters = value?.parameters;
    return parameters && typeof parameters === "object" && !Array.isArray(parameters) ? parameters : {};
  }

  function syncOptions(value) {
    draftProfile = typeof value?.profile === "string" ? value.profile : "";
    const parameters = parametersOf(value);
    parametersText = Object.keys(parameters).length ? JSON.stringify(parameters, null, 2) : "";
    validationError = "";
    applyError = "";
    dirty = false;
    applied = false;
  }

  // Sync a newly loaded selection while the draft is still pristine. Once the
  // person starts typing, a reload must not erase what they typed.
  $effect(() => {
    const source = options;
    if (source === observedOptions) return;
    observedOptions = source;
    if (!dirty) syncOptions(source);
  });

  // A parent echo of the options just saved is not a new request context.
  // Only a document/format scope change cancels pending work and resets drafts.
  $effect(() => {
    const key = scopeKey;
    if (key === observedScopeKey) return;
    observedScopeKey = key;
    applyRequest++;
    applying = false;
    syncOptions(options);
  });

  function parseDraft() {
    try {
      const parsed = parseRenderOptions({ format: "default", profile: draftProfile, parameters: parametersText });
      validationError = "";
      return parsed;
    } catch (error) {
      validationError = plain(error?.message);
      return null;
    }
  }

  // The parser's messages name the field for a programmer; say it the way
  // the page does.
  function plain(message) {
    const text = String(message || "").replace(/^Invalid Quarto render options: /, "");
    if (text.startsWith("profile must contain")) return "A profile name uses only letters, digits, dots, underscores and hyphens.";
    if (text === "parameters must be valid JSON") return "Parameters must be written as JSON, one name and value per line inside the braces.";
    if (text === "parameters must be a JSON object") return "Parameters must be an object in braces, like the example below.";
    if (text.startsWith("parameter must be a scalar")) return "Each parameter must be a single value: text in quotes, a number, true, false or null.";
    return text || "These settings are invalid.";
  }

  function changed() {
    dirty = true;
    applyError = "";
    applied = false;
    parseDraft();
  }

  let applied = $state(false);

  async function apply() {
    if (disabled || applying) return;
    const next = parseDraft();
    if (!next) return;
    applying = true;
    applied = false;
    const request = ++applyRequest;
    applyError = "";
    try {
      await onapplyoptions?.(next);
      if (request === applyRequest) {
        syncOptions(next);
        applied = true;
      }
    } catch (error) {
      if (request === applyRequest) applyError = error?.message || "Could not save these settings.";
    } finally {
      if (request === applyRequest) applying = false;
    }
  }

  const controlsDisabled = $derived(disabled || applying);
</script>

{#if disabled}<p class="setting-description options-disabled">Profile and parameter values are kept for a Quarto build. They cannot affect the current document's preview.</p>{/if}

<SettingRow id="rendering-profile" title="Profile" scope="This browser"
            description="Optional Quarto profile for this document, such as draft for _quarto-draft.yml.">
  <input class="input setting-input" type="text" value={draftProfile} placeholder="None"
         aria-label="Quarto profile" autocomplete="off" spellcheck="false"
         oninput={(event) => { draftProfile = event.currentTarget.value; changed(); }} disabled={controlsDisabled} />
</SettingRow>

<SettingRow id="rendering-parameters" title="Parameters" stacked scope="This browser"
            description="Optional JSON values for params declared in the document’s front matter.">
  <textarea class="textarea setting-textarea" rows="4" value={parametersText}
            placeholder={'{"year": 2026, "region": "north", "draft": true}'}
            aria-label="Quarto parameters" aria-invalid={Boolean(validationError)} spellcheck="false"
            aria-describedby={validationError || applyError ? "rendering-parameters-error" : undefined}
            oninput={(event) => { parametersText = event.currentTarget.value; changed(); }} disabled={controlsDisabled}></textarea>
  {#if validationError || applyError}
    <p id="rendering-parameters-error" class="setting-error" role="alert">{validationError || applyError}</p>
  {/if}
</SettingRow>

<style>
  .options-disabled { margin-block: calc(var(--spacing) * 3); }
</style>

<SettingRow title="" description={dirty ? "The preview restarts after saving." : ""}>
  <div class="setting-actions">
  <button id="rendering-save" class="btn btn-sm lp-control-brand" type="button" onclick={() => void apply()}
          disabled={controlsDisabled || !dirty || Boolean(validationError)}>
    {applying ? "Saving…" : "Save"}
  </button>
  {#if applied}<span class="setting-feedback" role="status">Saved</span>{/if}
  </div>
</SettingRow>
