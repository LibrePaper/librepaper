import { mount } from "svelte";
import Boundary from "../components/Boundary.svelte";
import { watchForUnhandled } from "../lib/crash.js";
import { watchNetwork } from "../lib/support/network.js";
import "../styles/app.css";
import { start as startTheme } from "../lib/theme.js";
import "../styles/reader.css";
import Reader from "../components/Reader.svelte";
import { registerOfflineShell } from "../lib/offline-shell.js";
import { intake } from "../lib/desktop/client.js";
import { blockPageZoom } from "../lib/pinch.js";
import { listenForSignOut } from "../lib/local-data.js";

startTheme();

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
// A signed-out browser must not keep writing the previous account's work.
listenForSignOut(() => location.replace("/"));
