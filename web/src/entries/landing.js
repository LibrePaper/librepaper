import { mount } from "svelte";
import Boundary from "../components/Boundary.svelte";
import { watchForUnhandled } from "../lib/crash.js";
import { watchNetwork } from "../lib/support/network.js";
import "../styles/app.css";
import { start as startTheme } from "../lib/theme.js";
import Landing from "../components/Landing.svelte";
import { registerOfflineShell } from "../lib/offline-shell.js";
import { intake } from "../lib/desktop/client.js";

startTheme();

// Mounted into <body> rather than into a wrapper: the stylesheet addresses
// the bar as `body > nav`, and an element in between would silently stop every
// one of those rules from matching.
// Started inside a boundary, so a throw anywhere below is a notice rather
// than an empty page. See src/components/Boundary.svelte.
watchForUnhandled();
watchNetwork();
const initialSettings = intake() ? "desktop" : "";
mount(Boundary, { target: document.body, props: { component: Landing, props: { initialSettings }, name: "the project list" } });
void registerOfflineShell();
