import { mount } from "svelte";
import Boundary from "../components/Boundary.svelte";
import { watchForUnhandled } from "../lib/crash.js";
import "../styles/app.css";
import Reader from "../components/Reader.svelte";
import { registerOfflineShell } from "../lib/offline-shell.js";

// Mounted into <body> rather than into a wrapper: the stylesheet addresses
// the bar as `body > nav`, and an element in between would silently stop every
// one of those rules from matching.
// Started inside a boundary, so a throw anywhere below is a notice rather
// than an empty page. See src/components/Boundary.svelte.
watchForUnhandled();
mount(Boundary, { target: document.body, props: { component: Reader, name: "the reader" } });
void registerOfflineShell();
