// Screenshots of the editor's visual mode, for comparing before and after a
// visual redesign by eye. This is a script, not a test: it asserts nothing and
// exits zero when the pictures were taken.
//
// It builds an entry with Vite that mounts web/src/components/Editor.svelte on
// a sample LaTeX article with the visual layer on, serves it over a local http
// server, and drives headless Chromium over CDP. For each variant it sets the
// palette, the light or dark mode, the viewport width, the editor font size and
// the caret, waits for fonts, KaTeX and images, grows the viewport to the whole
// document and writes `<variant>.png`. It also writes `metrics.json` for the
// default light shot: the content height and, in document order, the top, height,
// class list and first text of every `.cm-line` and block widget, so that empty
// lines which take vertical space can be found.
//
// Usage:
//   node web/tests/browser/visual-screenshots.mjs --out /some/dir
//
// Without --out the pictures go to a new temporary directory. The path of the
// output directory is printed at the end.

import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve as resolvePath, sep } from "node:path";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { requireChromiumExecutable } from "../helpers/browser-executable.mjs";
import { stopBrowserProcess, removeTemporary } from "../helpers/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-visual-screenshots-"));
const output = join(temporary, "build");
const profile = join(temporary, "chrome");
const entry = join(temporary, "entry.js");
// The Typst visual layer draws from typst-syntax.wasm's tree, so the page is offered the module.
const syntaxWasm = process.env.LIBREPAPER_TYPST_SYNTAX_WASM || join(root, "web/wasm/typst-syntax.wasm");
const katexDist = join(root, "web/node_modules/katex/dist");
// Any PNG of the repository will do as the figure the sample paper includes.
const iconFile = join(root, "web/public/icons/icon-512.png");
const port = 19000 + Math.floor(Math.random() * 1000);

// The pictures go where --out says, so a person can find them after the temporary build is gone.
const outFlag = process.argv.indexOf("--out");
const outDir = resolvePath(outFlag > 0 && process.argv[outFlag + 1] ? process.argv[outFlag + 1] : mkdtempSync(join(tmpdir(), "librepaper-visual-shots-")));
mkdirSync(outDir, { recursive: true });

// The article template with its title and author filled in, a captioned figure
// after the theorem and its proof, a second figure whose file is missing, and an
// itemize list with a nested enumerate in the related work.
const sample = String.raw`\documentclass[11pt]{article}

% Fonts and typography
\usepackage[T1]{fontenc}
\usepackage{lmodern}
\usepackage{microtype}
\usepackage[margin=1in]{geometry}

% Mathematics, figures and tables
\usepackage{amsmath,amssymb,amsthm}
\usepackage{graphicx}
\usepackage{booktabs}

% Citations: \citet{key} gives Author (Year), \citep{key} gives (Author, Year)
\usepackage[round,authoryear]{natbib}

% Links, then cross-references (cleveref must load after hyperref)
\usepackage{xcolor}
\usepackage[colorlinks=true,linkcolor=blue!50!black,citecolor=green!40!black,urlcolor=blue!50!black]{hyperref}
\usepackage[capitalise,noabbrev]{cleveref}

\newtheorem{theorem}{Theorem}

% Edit the title, author and affiliation below
\title{A Sample Paper}
\author{Ada Lovelace\thanks{Department of Statistics, University of Example. \href{mailto:author@example.org}{author@example.org}}}
\date{\today}

\begin{document}
\maketitle

\begin{abstract}
  We study how a simple linear model recovers a treatment effect from a randomized experiment. We compare three estimators in a small simulation and report their bias and coverage. The results favour regression adjustment when the outcome has a strong predictor, and the bootstrap gives reliable intervals in both settings.
\end{abstract}

\noindent\textbf{Keywords:} causal inference, regression adjustment, bootstrap, simulation

\section{Introduction}\label{sec:introduction}

Information theory \citep{shannon1948} and resampling \citep{efron1979} remain the workhorses of statistical inference. This paper asks how much a simple model can recover from a randomized experiment, and how much an analyst should trust the resulting intervals.

Work on the potential outcomes framework \citet{rubin1974} and on ensemble methods \citep{breiman2001} motivates the design of the study. Our contributions are threefold. First, we state the identification result in \cref{sec:model}. Second, we compare three estimators in \cref{sec:results}. Third, we discuss when each estimator should be preferred in \cref{sec:conclusion}.

\section{Related work}\label{sec:related}

Early treatments of experimental design focus on randomization as the source of identification. Modern practice adds covariates to gain precision, a step that is now standard in applied work \citep{rubin1974}. Resampling methods provide intervals without distributional assumptions \citep{efron1979}, and they have been studied extensively for ensembles of trees \citep{breiman2001}. The literature falls into a few strands:
\begin{itemize}
  \item Design-based inference, which rests on the randomization alone.
  \item Model-based inference, which adds assumptions in exchange for precision:
  \begin{enumerate}
    \item linear adjustment for baseline covariates;
    \item flexible adjustment with ensembles of trees.
  \end{enumerate}
  \item Resampling, which replaces distributional assumptions with computation.
\end{itemize}

\section{Model}\label{sec:model}

Let $Y_i$ denote the outcome for unit $i$ and let $D_i \in \{0,1\}$ indicate treatment. We assume the linear model
\begin{equation}\label{eq:model}
  Y_i = \beta_0 + \beta_1 D_i + \varepsilon_i,
  \qquad
  \mathbb{E}[\varepsilon_i \mid D_i] = 0.
\end{equation}
Under \cref{eq:model}, the difference in means recovers the treatment effect, as stated in the following result.

\begin{theorem}\label{thm:identification}
  Suppose \cref{eq:model} holds and treatment is randomly assigned, so that $\varepsilon_i$ is independent of $D_i$. Then $\beta_1 = \mathbb{E}[Y_i \mid D_i = 1] - \mathbb{E}[Y_i \mid D_i = 0]$.
\end{theorem}

\begin{proof}
  Take conditional expectations of \cref{eq:model} given $D_i$. The error term has mean zero in each arm, so the difference in the two conditional means equals $\beta_1$.
\end{proof}

\begin{figure}[ht]
  \centering
  \includegraphics[width=0.4\linewidth]{icon.png}
  \caption{The LibrePaper icon, loaded from the project.}\label{fig:icon}
\end{figure}

\begin{figure}[ht]
  \centering
  \includegraphics[width=0.4\linewidth]{missing.png}
  \caption{A figure whose file is not in the project.}\label{fig:missing}
\end{figure}

\section{Results}\label{sec:results}

\cref{tab:results} reports the estimates from a simulation with 500 replications. Each cell gives the mean estimate with its standard error in parentheses, and the coverage of the nominal 95\% interval.

\begin{table}[ht]
  \centering
  \caption{Estimated treatment effect by estimator. Standard errors in parentheses.}\label{tab:results}
  \begin{tabular}{lcc}
    \toprule
    Estimator & Estimate & Coverage \\
    \midrule
    Difference in means & 0.52 (0.11) & 0.94 \\
    Regression adjustment & 0.49 (0.08) & 0.95 \\
    Bootstrap percentile & 0.50 (0.09) & 0.93 \\
    \bottomrule
  \end{tabular}
\end{table}

Regression adjustment has the smallest standard error, as expected from the variance reduction it provides. The bootstrap intervals are slightly narrower than the normal approximation and still cover close to the nominal rate.

\section{Conclusion}\label{sec:conclusion}

Under randomization, a linear model gives a transparent route to the treatment effect. Regression adjustment improves precision at little cost, and the bootstrap is a sound check when the distribution of the estimator is unclear. Future work should extend the comparison to clustered designs.

\bibliographystyle{plainnat}
\bibliography{references}

\end{document}
`;

// Runs in the page. It refers only to names the generated entry defines (tick,
// EditorView, component, setPalette, setChoice), so its source is copied into
// the entry as it stands.
//
// Waits until the page has stopped changing: fonts are loaded, every image has
// finished, and the editor's markup has kept the same size for several frames.
// Deferred rebuilds (formulas, labels) land within that time, so no fixed sleep
// is needed.
async function settle() {
  const frame = () => new Promise((done) => requestAnimationFrame(() => done()));
  await tick();
  await document.fonts.ready;
  let stable = 0;
  let last = -1;
  const started = performance.now();
  while (stable < 12 && performance.now() - started < 10000) {
    await frame();
    const images = Array.from(document.querySelectorAll(".cm-editor img"));
    const loading = images.some((image) => !image.complete);
    const size = document.querySelector(".cm-content").innerHTML.length + document.querySelector(".cm-content").getBoundingClientRect().height;
    stable = !loading && size === last ? stable + 1 : 0;
    last = size;
  }
  await document.fonts.ready;
}

// Runs in the page. Puts the editor in one state: palette, mode, visual layer,
// font size in pixels (null for the default) and the caret ("body" parks it on
// the last line, "preamble" puts it on line 1).
async function arrange({ palette, mode, visual, fontSize, caret }) {
  setPalette(palette);
  setChoice(mode);
  component.$set({ visual });
  const editor = document.querySelector(".cm-editor");
  // The editor reads its size from this variable, which the zoom code sets on the host.
  if (fontSize) editor.style.setProperty("--editor-font-size", `${fontSize}px`);
  else editor.style.removeProperty("--editor-font-size");
  await settle();
  const view = EditorView.findFromDOM(editor);
  const text = view.state.doc.toString();
  const anchor = caret === "preamble" ? 0 : text.indexOf("\\end{document}") + 2;
  view.dispatch({ selection: { anchor } });
  await settle();
}

// Runs in the page. The height, in pixels, from the top of the scroller to the bottom of the content.
function contentHeight() {
  const scroller = document.querySelector(".cm-scroller");
  const content = document.querySelector(".cm-content");
  return Math.ceil(content.getBoundingClientRect().bottom - scroller.getBoundingClientRect().top + scroller.scrollTop) + 8;
}

// Runs in the page. Every direct child of the content (a line or a block
// widget) in document order, with where it sits and what it holds.
function collectMetrics() {
  const content = document.querySelector(".cm-content");
  const origin = content.getBoundingClientRect().top;
  const items = Array.from(content.children)
    .filter((node) => !node.classList.contains("cm-widgetBuffer"))
    .map((node) => {
      const rect = node.getBoundingClientRect();
      return {
        kind: node.classList.contains("cm-line") ? "line" : "block",
        top: Math.round((rect.top - origin) * 100) / 100,
        height: Math.round(rect.height * 100) / 100,
        classes: Array.from(node.classList),
        text: (node.textContent || "").trim().slice(0, 40),
        // Figure lines are where hidden source and widgets share a line, so their
        // markup and computed box are kept for debugging the layout.
        ...(node.classList.contains("cm-lv-figure")
          ? {
              html: node.outerHTML.slice(0, 2000),
              style: (({ display, lineHeight, paddingTop, paddingBottom, flexDirection, alignItems }) => ({ display, lineHeight, paddingTop, paddingBottom, flexDirection, alignItems }))(getComputedStyle(node)),
              children: Array.from(node.children).map((child) => ({ tag: child.tagName, classes: child.className, height: Math.round(child.getBoundingClientRect().height), left: Math.round(child.getBoundingClientRect().left), width: Math.round(child.getBoundingClientRect().width), display: getComputedStyle(child).display })),
            }
          : {}),
      };
    });
  return { contentHeight: Math.round(content.getBoundingClientRect().height * 100) / 100, items };
}

const pageSource = [
  `import ${JSON.stringify(join(root, "web/src/styles/app.css"))};`,
  `import { tick } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};`,
  `import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};`,
  `import { join as joinSession } from ${JSON.stringify(join(root, "web/src/lib/collab.js"))};`,
  `import { setPalette, setChoice } from ${JSON.stringify(join(root, "web/src/lib/theme.js"))};`,
  `import Editor from ${JSON.stringify(join(root, "web/src/components/Editor.svelte"))};`,
  `import { createClassComponent } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/legacy/legacy-client.js"))};`,
  `globalThis.LIBREPAPER_MODULES = { "typst-syntax": "/typst-syntax.wasm" };`,
  `document.documentElement.dataset.theme = "librepaper";`,
  `const host = document.createElement("main");`,
  `host.style.cssText = "height: 100vh; width: 100vw; display: flex; flex-direction: column; overflow: hidden;";`,
  `document.body.append(host);`,
  `const session = joinSession({ send: () => {}, mayEdit: true });`,
  `const file = session.addText("main.tex", ${JSON.stringify(sample)});`,
  // The resolver answers like the app's: a URL for a file in the project, null for one that is not.
  `const imageUrl = async (path) => (path === "icon.png" || path === "missing.png" ? "/" + path : null);`,
  `const component = createClassComponent({`,
  `  component: Editor,`,
  `  target: host,`,
  `  props: { session, format: "latex", file, visual: true, imageUrl },`,
  `});`,
  `window.settle = ${settle.toString()};`,
  `window.arrange = ${arrange.toString()};`,
  `window.contentHeight = ${contentHeight.toString()};`,
  `window.collectMetrics = ${collectMetrics.toString()};`,
  `window.shotsReady = true;`,
  "",
].join("\n");

// The variants, in the order they are taken. Each name is the picture's file name.
const palettes = ["librepaper", "flexoki", "solarized", "gruvbox", "catppuccin", "nord"];
const variants = [];
for (const palette of palettes) {
  for (const mode of ["light", "dark"]) {
    variants.push({ name: `visual-${palette === "librepaper" ? "default" : palette}-${mode}`, palette, mode, visual: true, width: 1280, caret: "body" });
  }
}
variants.push(
  { name: "visual-narrow", palette: "librepaper", mode: "light", visual: true, width: 520, caret: "body" },
  // 14px is the default size; 1.3 times that is about 18px.
  { name: "visual-large-font", palette: "librepaper", mode: "light", visual: true, width: 1280, fontSize: 18, caret: "body" },
  { name: "visual-preamble-open", palette: "librepaper", mode: "light", visual: true, width: 1280, caret: "preamble" },
  { name: "source-default-light", palette: "librepaper", mode: "light", visual: false, width: 1280, caret: "body" },
);

writeFileSync(entry, pageSource);
let server;
let browser;
let socket;
const errors = [];
try {
  await build({
    configFile: false,
    root: join(root, "web"),
    define: { __KATEX__: JSON.stringify("/katex/") },
    resolve: { alias: loroAlias },
    plugins: [tailwindcss(), svelte()],
    logLevel: "error",
    build: {
      outDir: output,
      emptyOutDir: true,
      lib: { entry, formats: ["es"], fileName: () => "visual-screenshots.js", cssFileName: "visual-screenshots" },
    },
  });

  // KaTeX is served where the build's __KATEX__ points; icon.png is a real PNG
  // and missing.png answers 404, so the page shows both a drawn and a failed figure.
  server = createServer((request, response) => {
    const path = decodeURIComponent(request.url.split("?")[0]);
    if (path === "/typst-syntax.wasm") {
      response.setHeader("Content-Type", "application/wasm");
      response.end(readFileSync(syntaxWasm));
      return;
    }
    if (path === "/icon.png") {
      response.setHeader("Content-Type", "image/png");
      response.end(readFileSync(iconFile));
      return;
    }
    if (path === "/missing.png") {
      response.statusCode = 404;
      response.end();
      return;
    }
    if (path.startsWith("/katex/")) {
      const file = join(katexDist, path.slice("/katex/".length));
      if (file.startsWith(katexDist + sep) && existsSync(file)) {
        response.setHeader("Content-Type", file.endsWith(".woff2") ? "font/woff2" : contentType(file));
        response.end(readFileSync(file));
        return;
      }
      response.statusCode = 404;
      response.end();
      return;
    }
    const file = join(output, path.slice(1));
    if (path !== "/" && existsSync(file)) {
      response.setHeader("Content-Type", contentType(file));
      response.end(readFileSync(file));
      return;
    }
    response.setHeader("Content-Type", "text/html");
    response.end('<!doctype html><body><link rel="stylesheet" href="/visual-screenshots.css"><script type="module" src="/visual-screenshots.js"></script></body>');
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  const httpPort = typeof address === "object" && address ? address.port : 0;
  if (!httpPort) throw new Error("local screenshot server did not start");

  browser = spawn(requireChromiumExecutable(), [
    "--headless=new",
    "--window-size=1280,2000",
    "--no-sandbox",
    "--disable-gpu",
    `--user-data-dir=${profile}`,
    `--remote-debugging-port=${port}`,
    "about:blank",
  ], { stdio: "ignore", detached: true });
  const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
  let browserInfo;
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      browserInfo = await fetch(`http://127.0.0.1:${port}/json`).then((answer) => answer.json());
      break;
    } catch {
      await wait(100);
    }
  }
  const page = browserInfo?.find((target) => target.type === "page");
  if (!page) throw new Error("Chromium did not expose a page target");
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });

  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    if (message.method === "Runtime.exceptionThrown") {
      const details = message.params.exceptionDetails;
      errors.push(details.exception?.description ?? details.text);
    }
    const resolve = pending.get(message.id);
    if (resolve) {
      pending.delete(message.id);
      resolve(message);
    }
  });
  const send = (method, params = {}) => new Promise((resolve) => {
    const id = ++sequence;
    pending.set(id, resolve);
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async (expression) => {
    const message = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (message.result?.exceptionDetails) throw new Error(JSON.stringify(message.result.exceptionDetails));
    return message.result.result.value;
  };
  const resize = (width, height) => send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });

  await send("Runtime.enable");
  await send("Page.enable");
  await send("Page.navigate", { url: `http://127.0.0.1:${httpPort}/` });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    ready = await evaluate("Boolean(window.shotsReady && document.querySelector('.cm-editor'))");
    if (ready) break;
    await wait(100);
  }
  if (!ready) throw new Error("Editor did not mount in Chromium");

  for (const variant of variants) {
    await resize(variant.width, 2000);
    await evaluate(`arrange(${JSON.stringify(variant)})`);
    // CodeMirror draws only the lines near the window, and the heights of the
    // rest are estimates. Growing the window to the measured height draws more
    // of them, so the height is measured again until it stops moving.
    let height = 2000;
    for (let attempt = 0; attempt < 4; attempt++) {
      const wanted = Math.max(400, await evaluate("contentHeight()"));
      if (wanted === height) break;
      height = wanted;
      await resize(variant.width, height);
      await evaluate("settle()");
    }
    if (variant.name === "visual-default-light") {
      const metrics = await evaluate("collectMetrics()");
      writeFileSync(join(outDir, "metrics.json"), JSON.stringify({ viewportWidth: variant.width, viewportHeight: height, ...metrics }, null, 2));
    }
    const shot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: true });
    if (!shot.result?.data) throw new Error(`no screenshot data for ${variant.name}: ${JSON.stringify(shot.error ?? shot)}`);
    writeFileSync(join(outDir, `${variant.name}.png`), Buffer.from(shot.result.data, "base64"));
    console.log(`visual-screenshots: ${variant.name}.png (${variant.width}x${height})`);
  }

  if (errors.length) console.log(["visual-screenshots: browser exceptions:", ...errors].join("\n"));
  console.log(`visual-screenshots: wrote ${variants.length} pictures and metrics.json to ${outDir}`);
} finally {
  socket?.close();
  await stopBrowserProcess(browser);
  if (server?.listening) await new Promise((resolve) => server.close(resolve));
  removeTemporary(temporary);
}
