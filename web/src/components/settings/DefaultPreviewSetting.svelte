<script>
  // The preview every document of one format opens on, for this person in
  // this browser. View > HTML and View > PDF change it for one visit only.
  import SettingRow from "./SettingRow.svelte";
  import { readDefaultPreview, writeDefaultPreview } from "../../lib/build-preferences.js";
  import { m } from "../../lib/paraglide/messages.js";

  let { format, userId = "anonymous", onpreferences } = $props();

  const NAMES = { latex: "LaTeX", typst: "Typst", markdown: "Markdown", quarto: "Quarto" };
  const DESCRIPTIONS = {
    latex: m.defaultpreview_description({ format: "LaTeX" }),
    typst: m.defaultpreview_description({ format: "Typst" }),
    markdown: m.defaultpreview_description({ format: "Markdown" }),
    quarto: m.defaultpreview_description({ format: "Quarto" }),
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

<SettingRow id={`render-${format}-preview`} title={m.defaultpreview_title()} description={DESCRIPTIONS[format]}>
  <select class="select setting-select" aria-label={m.defaultpreview_aria({ name })} value={value}
          onchange={(event) => choose(event.currentTarget.value)}>
    <option value="html">HTML</option>
    <option value="pdf">PDF</option>
  </select>
</SettingRow>
