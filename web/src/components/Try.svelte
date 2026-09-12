<script>
  import { onMount } from "svelte";
  import { EditorState } from "@codemirror/state";
  import { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection } from "@codemirror/view";
  import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
  import { markdown } from "@codemirror/lang-markdown";
  import { syntaxHighlighting, defaultHighlightStyle } from "@codemirror/language";
  import Logo from "./Logo.svelte";
  import * as renderers from "../lib/renderers.js";

  const EXAMPLE = `# A paper you can change

LibrePaper lets authors and reviewers work on the same source. **Edit this sentence** and the paper beside it will update.

## Comments that keep their place

Comments attach to the passage they discuss, rather than to a page number. When the paper changes, the conversation can follow the words.

## Mathematics and citations

Markdown remains plain text while the rendered paper can contain structure, links, and mathematics such as $E = mc^2$.

> This playground stays in this tab. Reload it and everything starts over.
`;

  let editorHost;
  let editor;
  let preview = $state("");
  let problem = $state("");
  let rendering = $state(true);
  let generation = 0;
  let timer;

  async function render(source) {
    const current = ++generation;
    rendering = true;
    try {
      const result = await renderers.render(
        { main: "paper.md", texts: { "paper.md": source }, assets: {} },
        "A paper you can change",
        { format: "html" },
      );
      if (current !== generation) return;
      if (!result?.html) throw new Error("The Markdown renderer returned no page.");
      preview = result.html;
      problem = "";
    } catch (error) {
      if (current === generation) problem = error?.message || "The preview could not be rendered.";
    } finally {
      if (current === generation) rendering = false;
    }
  }

  function schedule(source) {
    clearTimeout(timer);
    timer = setTimeout(() => render(source), 120);
  }

  function reset() {
    editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: EXAMPLE } });
  }

  onMount(() => {
    editor = new EditorView({
      parent: editorHost,
      state: EditorState.create({
        doc: EXAMPLE,
        extensions: [
          lineNumbers(),
          highlightActiveLine(),
          drawSelection(),
          history(),
          markdown(),
          syntaxHighlighting(defaultHighlightStyle),
          keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
          EditorView.lineWrapping,
          EditorView.updateListener.of((update) => {
            if (update.docChanged) schedule(update.state.doc.toString());
          }),
        ],
      }),
    });
    void render(EXAMPLE);
    return () => {
      clearTimeout(timer);
      generation += 1;
      editor.destroy();
    };
  });
</script>

<svelte:head><meta name="robots" content="noindex" /></svelte:head>

<nav class="border-surface-200-800 flex items-center justify-between gap-4 border-b px-4 py-3">
  <a class="flex items-center gap-2" href="https://librepaper.org" aria-label="LibrePaper home"><Logo /></a>
  <a class="btn btn-sm preset-filled-primary-500" href="/auth/login?next=%2F">Sign in to publish</a>
</nav>

<main class="playground">
  <header class="playground-intro">
    <div>
      <p class="eyebrow">Temporary playground</p>
      <h1>Change the source. See the paper change.</h1>
      <p>Everything stays in this tab. Nothing is uploaded, saved, or shareable. Reloading starts over.</p>
    </div>
    <button class="btn btn-sm preset-outlined-surface-300-700" type="button" onclick={reset}>Reset example</button>
  </header>

  <section class="workspace" aria-label="LibrePaper playground">
    <div class="pane source-pane">
      <div class="pane-header"><strong>paper.md</strong><span>Markdown</span></div>
      <div class="editor" bind:this={editorHost}></div>
    </div>
    <div class="pane preview-pane">
      <div class="pane-header">
        <strong>Paper</strong>
        <span>{rendering ? "Rendering…" : "Preview"}</span>
      </div>
      {#if problem}
        <div class="problem" role="alert">{problem}</div>
      {:else if preview}
        <iframe title="Rendered paper" sandbox="" srcdoc={preview}></iframe>
      {/if}
    </div>
  </section>
</main>

<style>
  :global(body) { min-height: 100vh; margin: 0; overflow: hidden; }
  .playground { height: calc(100vh - 57px); display: flex; flex-direction: column; background: var(--color-surface-50-950); }
  .playground-intro { display: flex; align-items: center; justify-content: space-between; gap: 1.5rem; padding: 1.1rem 1.5rem; border-bottom: 1px solid var(--color-surface-200-800); }
  .playground-intro h1 { margin: .1rem 0 .2rem; font-size: clamp(1.25rem, 2.4vw, 1.75rem); font-weight: 650; }
  .playground-intro p { margin: 0; color: var(--color-surface-600-400); }
  .playground-intro .eyebrow { color: var(--color-primary-600-400); font-size: .72rem; font-weight: 700; letter-spacing: .08em; text-transform: uppercase; }
  .workspace { min-height: 0; flex: 1; display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); }
  .pane { min-width: 0; min-height: 0; display: flex; flex-direction: column; background: var(--color-surface-50-950); }
  .source-pane { border-right: 1px solid var(--color-surface-200-800); }
  .pane-header { height: 2.6rem; flex: none; display: flex; align-items: center; justify-content: space-between; padding: 0 .9rem; border-bottom: 1px solid var(--color-surface-200-800); font-size: .8rem; }
  .pane-header span { color: var(--color-surface-500); }
  .editor { min-height: 0; flex: 1; overflow: auto; }
  .editor :global(.cm-editor) { height: 100%; font-size: .95rem; }
  .editor :global(.cm-scroller) { font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace; line-height: 1.65; }
  .editor :global(.cm-content) { padding: 1rem 0; }
  .editor :global(.cm-gutters) { background: transparent; border: 0; color: var(--color-surface-400); }
  .preview-pane iframe { width: 100%; min-height: 0; flex: 1; border: 0; background: white; }
  .problem { margin: 1rem; padding: 1rem; border-radius: .5rem; background: var(--color-error-50); color: var(--color-error-700); }
  @media (max-width: 760px) {
    :global(body) { overflow: auto; }
    .playground { height: auto; min-height: calc(100vh - 57px); }
    .playground-intro { align-items: flex-start; padding: 1rem; }
    .workspace { grid-template-columns: 1fr; grid-template-rows: minmax(22rem, 48vh) minmax(28rem, 60vh); }
    .source-pane { border-right: 0; border-bottom: 1px solid var(--color-surface-200-800); }
  }
</style>
