import { mount } from "svelte";
import Boundary from "../components/Boundary.svelte";
import { watchForUnhandled } from "../lib/crash.js";
import { watchNetwork } from "../lib/support/network.js";
import "../styles/app.css";
import { start as startTheme } from "../lib/theme.js";
import { start as startEditorType } from "../lib/editor-type.js";
import "../styles/reader.css";
import Reader from "../components/Reader.svelte";
import { registerOfflineShell } from "../lib/offline-shell.js";
import { intake } from "../lib/desktop/client.js";
import { blockPageZoom } from "../lib/pinch.js";
import { listenForSignOut } from "../lib/local-data.js";
import { signedOutHref } from "../lib/api.js";
import { markDocumentLanguage } from "../lib/locale.js";
import { installTips } from "../lib/tip.js";
import loroWasmUrl from "loro-crdt/web/loro_wasm_bg.wasm?url";

markDocumentLanguage();
// The one tooltip, for every element that carries data-tip.
installTips();

// The editor font URLs arrive the same way and are read first, because the
// font is applied as soon as the editor type starts.
globalThis.LIBREPAPER_FONTS = JSON.parse(document.getElementById("librepaper-fonts").textContent);

startTheme();
// The settings can be open before any editor is, and still follow another tab.
startEditorType();

// Mounted into <body> rather than into a wrapper: the stylesheet addresses
// the bar as `body > nav`, and an element in between would silently stop every
// one of those rules from matching.
// Started inside a boundary, so a throw anywhere below is a notice rather
// than an empty page. See src/components/Boundary.svelte.
watchForUnhandled();
watchNetwork();

// A pinch on the chrome should do nothing rather than magnify the toolbar. The
// document frames zoom their own content, so this only covers the application.
blockPageZoom(document);

// The renderer URLs arrive as a JSON data block, not an inline script: the
// page's policy refuses inline script. Read before anything renders, because
// the reader asks which formats it can render as it mounts.
globalThis.LIBREPAPER_MODULES = JSON.parse(document.getElementById("librepaper-modules").textContent);

const initialSettings = intake() ? "tools" : "";
mount(Boundary, { target: document.body, props: { component: Reader, props: { initialSettings }, name: "the reader" } });
void registerOfflineShell();
// The CRDT's wasm is fetched only once a session starts, which is after the
// document has been asked for. Hinting it here lets the browser fetch it at
// idle priority while the page is still waiting on that answer, so an editor
// opens without three quarters of a megabyte on its critical path. It is only
// downloaded, never compiled, for someone who turns out to be a reader.
const loroHint = document.createElement("link");
loroHint.rel = "prefetch";
loroHint.href = loroWasmUrl;
document.head.append(loroHint);
// A signed-out browser must not keep writing the previous account's work.
listenForSignOut(() => location.replace(signedOutHref()));
