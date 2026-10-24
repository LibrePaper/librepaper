<script>
  // The preview every document of one format opens on, for this person in
  // this browser. View > HTML and View > PDF change it for one visit only.
  import SettingRow from "./SettingRow.svelte";
  import { readDefaultPreview, writeDefaultPreview } from "../../lib/build-preferences.js";

  let { format, userId = "anonymous", onpreferences } = $props();

  const NAMES = { latex: "LaTeX", typst: "Typst", markdown: "Markdown", quarto: "Quarto" };
  const DESCRIPTIONS = {
    latex: "Every LaTeX document opens on this preview. View > HTML and View > PDF switch it until you leave the project.",
    typst: "Every Typst document opens on this preview. View > HTML and View > PDF switch it until you leave the project.",
    markdown: "Every Markdown document opens on this preview. View > HTML and View > PDF switch it until you leave the project.",
    quarto: "Every Quarto document opens on this preview. View > HTML and View > PDF switch it until you leave the project.",
  };

  const name = $derived(NAMES[format]);
  const scope = () => ({ origin: globalThis.location?.origin || "", user: userId });
  let value = $state("html");
  $effect.pre(() => { value = readDefaultPreview(scope(), format); });

  function choose(next) {
    const preference = writeDefaultPreview(scope(), format, next);
    value = next;
    onpreferences?.(format, preference);
  }
</script>

<SettingRow id={`render-${format}-preview`} title="Default preview" description={DESCRIPTIONS[format]}>
  <select class="select setting-select" aria-label={`${name} default preview`} value={value}
          onchange={(event) => choose(event.currentTarget.value)}>
    <option value="html">HTML</option>
    <option value="pdf">PDF</option>
  </select>
</SettingRow>
