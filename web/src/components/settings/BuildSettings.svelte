<script>
  // Which LaTeX engine builds every LaTeX document in this browser. The choice
  // is global: it is read for the format, not for the document open.
  import SettingRow from "./SettingRow.svelte";
  import { defaults, read, update } from "../../lib/build-preferences.js";

  let { format = "latex", userId = "anonymous", onpreferences } = $props();

  // The browser compiler ships pdfTeX and XeTeX only, so those are the
  // engines; the first choice is what an untouched preference means.
  const ROW = {
    id: "render-latex-engine", title: "Engine", label: "LaTeX engine",
    description: "Automatic reads a % !TEX program line, then the packages the document loads.",
    choices: [
      { value: "automatic", says: "Automatic", patch: { selection: "automatic" } },
      { value: "pdflatex", says: "pdfLaTeX", patch: { selection: "tool", engine: "pdflatex" } },
      { value: "xelatex", says: "XeLaTeX", patch: { selection: "tool", engine: "xelatex" } },
    ],
    chosen: (preference) => preference.selection === "tool" && ["pdflatex", "xelatex"].includes(preference.engine) ? preference.engine : "automatic",
  };

  const scope = () => ({ origin: globalThis.location?.origin || "", user: userId });
  /** @type {import("../../lib/reader/build-settings.svelte.js").BuildPreference} */
  let preference = $state(defaults());
  $effect.pre(() => { preference = read(scope(), format); });

  function choose(value) {
    const choice = ROW.choices.find((each) => each.value === value);
    preference = update(scope(), format, choice.patch);
    onpreferences?.(format, preference);
  }
</script>

<SettingRow id={ROW.id} title={ROW.title} description={ROW.description}>
  <select class="select setting-select" aria-label={ROW.label} value={ROW.chosen(preference)}
          onchange={(event) => choose(event.currentTarget.value)}>
    {#each ROW.choices as choice (choice.value)}<option value={choice.value}>{choice.says}</option>{/each}
  </select>
</SettingRow>
