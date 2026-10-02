<script>
  // Which renderer builds every document of one format in this browser. The
  // choice is global: it is read for the format, not for the document open.
  import SettingRow from "./SettingRow.svelte";
  import * as localBridge from "../../lib/companion/client.js";
  import { read, update } from "../../lib/build-preferences.js";

  let { format, userId = "anonymous", onpreferences } = $props();

  const browser = (tool) => ({ selection: "tool", backend: "browser", tool });
  const local = (tool) => ({ selection: "tool", backend: "local", tool });
  // The browser compiler ships pdfTeX and XeTeX only, so those are the
  // engines; the first choice of each row is what an untouched preference means.
  const ROWS = {
    latex: {
      id: "render-latex-engine", title: "Engine", label: "LaTeX engine",
      description: "Automatic follows a % !TEX program line, then the packages the document loads.",
      choices: [
        { value: "automatic", says: "Automatic", patch: { selection: "automatic" } },
        { value: "pdflatex", says: "pdfLaTeX", patch: { ...browser("tex"), engine: "pdflatex" } },
        { value: "xelatex", says: "XeLaTeX", patch: { ...browser("tex"), engine: "xelatex" } },
      ],
      chosen: (preference) => preference.selection === "tool" && ["pdflatex", "xelatex"].includes(preference.engine) ? preference.engine : "automatic",
    },
    typst: {
      id: "render-typst-tool", title: "Render with", label: "Typst renderer",
      description: "Calepin runs on this computer once local code execution is on for the document.",
      choices: [
        { value: "browser", says: "Browser", patch: browser("typst") },
        { value: "calepin", says: "Calepin (local)", patch: local("calepin") },
      ],
      chosen: (preference) => preference.backend === "local" && preference.tool === "calepin" ? "calepin" : "browser",
    },
    markdown: {
      id: "render-markdown-tool", title: "Markdown files", label: "Markdown renderer",
      description: "Pandoc and Quarto run on this computer through the companion.",
      choices: [
        { value: "browser", says: "Browser", patch: browser("markdown") },
        { value: "pandoc", says: "Pandoc (local)", patch: local("pandoc") },
        { value: "quarto", says: "Quarto (local)", patch: local("quarto") },
      ],
      chosen: (preference) => preference.backend === "local" && ["pandoc", "quarto"].includes(preference.tool) ? preference.tool : "browser",
    },
    quarto: {
      id: "render-quarto-tool", title: "Quarto files", label: "Quarto renderer",
      description: "Quarto runs on this computer once local code execution is on for the document.",
      choices: [
        { value: "browser", says: "Browser", patch: browser("markdown") },
        { value: "quarto", says: "Quarto (local)", patch: local("quarto") },
      ],
      chosen: (preference) => preference.backend === "local" && preference.tool === "quarto" ? "quarto" : "browser",
    },
  };

  const row = $derived(ROWS[format]);
  const scope = () => ({ origin: globalThis.location?.origin || "", user: userId });
  let preference = $state({});
  $effect.pre(() => { preference = read(scope(), format); });

  function choose(value) {
    const choice = row.choices.find((each) => each.value === value);
    preference = update(scope(), format, choice.patch);
    onpreferences?.(format, preference);
    // Choosing a local tool is the gesture that may reach the companion, so
    // the Local tools row can then say whether that tool is there.
    if (choice.patch.backend === "local") void localBridge.probe({ force: true });
  }
</script>

<SettingRow id={row.id} title={row.title} description={row.description}>
  <select class="select setting-select" aria-label={row.label} value={row.chosen(preference)}
          onchange={(event) => choose(event.currentTarget.value)}>
    {#each row.choices as choice (choice.value)}<option value={choice.value}>{choice.says}</option>{/each}
  </select>
</SettingRow>
