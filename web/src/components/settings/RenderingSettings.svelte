<script>
  // Two things a Quarto project may take when it is previewed here: a profile
  // and parameters. Both are optional, both are remembered per document in
  // this browser only, and a project that declares neither needs nothing on
  // this page. There is no format choice: the live preview is always the
  // page Quarto renders, in whatever format the document's front matter says.
  import SettingRow from "./SettingRow.svelte";
  import { parseRenderOptions } from "../../lib/quarto-options.js";

  let { options, viewing = null, onapplyoptions } = $props();

  let draftProfile = $state("");
  let parametersText = $state("{}");
  let validationError = $state("");
  let applyError = $state("");
  let applying = $state(false);
  let dirty = $state(false);
  let observedOptions = null;

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
  }

  // Sync a newly loaded selection while the draft is still pristine. Once the
  // person starts typing, a reload must not erase what they typed.
  $effect(() => {
    const source = options;
    if (source === observedOptions) return;
    observedOptions = source;
    if (!dirty) syncOptions(source);
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
    parseDraft();
  }

  async function apply() {
    if (disabled || applying) return;
    const next = parseDraft();
    if (!next) return;
    applying = true;
    applyError = "";
    try {
      await onapplyoptions?.(next);
      syncOptions(next);
    } catch (error) {
      applyError = error?.message || "Could not apply these settings.";
    } finally {
      applying = false;
    }
  }

  const disabled = $derived(Boolean(viewing) || applying);
</script>

<SettingRow id="rendering-profile" title="Profile"
            description="Leave this empty unless your project has Quarto profiles (files named _quarto-something.yml). Then type the name of the one to preview with, such as “draft” for _quarto-draft.yml.">
  <input class="input setting-input" type="text" value={draftProfile} placeholder="None"
         aria-label="Quarto profile" autocomplete="off" spellcheck="false"
         oninput={(event) => { draftProfile = event.currentTarget.value; changed(); }} {disabled} />
</SettingRow>

<SettingRow id="rendering-parameters" title="Parameters" stacked
            description="Leave this empty unless the document declares params: in its front matter. Then give the values to preview with, as JSON: a name in quotes, a colon, and a value.">
  <textarea class="textarea setting-textarea" rows="4" value={parametersText}
            placeholder={'{"year": 2026, "region": "north", "draft": true}'}
            aria-label="Quarto parameters" aria-invalid={Boolean(validationError)} spellcheck="false"
            oninput={(event) => { parametersText = event.currentTarget.value; changed(); }} {disabled}></textarea>
  {#if validationError || applyError}
    <p class="setting-error" role="alert">{validationError || applyError}</p>
  {/if}
</SettingRow>

<SettingRow title="" description={viewing ? "Return to the current version to change these." : dirty ? "The preview restarts with the new settings." : ""}>
  <button class="btn btn-sm preset-filled-primary-500" type="button" onclick={() => void apply()}
          disabled={disabled || !dirty || Boolean(validationError)}>
    {applying ? "Applying…" : "Apply"}
  </button>
</SettingRow>
