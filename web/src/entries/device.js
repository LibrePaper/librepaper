import { mount } from "svelte";
import Boundary from "../components/Boundary.svelte";
import { watchForUnhandled } from "../lib/crash.js";
import { watchNetwork } from "../lib/support/network.js";
import "../styles/app.css";
import { start as startTheme } from "../lib/theme.js";
import Device from "../components/Device.svelte";

startTheme();

// Mounted into <body> rather than into a wrapper, for the same reason the
// sign-in page is: the stylesheet addresses the bar as `body > nav`.
// Started inside a boundary, so a throw anywhere below is a notice rather
// than an empty page. See src/components/Boundary.svelte.
watchForUnhandled();
watchNetwork();
mount(Boundary, { target: document.body, props: { component: Device, name: "device sign-in" } });
