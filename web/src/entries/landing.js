import { mount } from "svelte";
import "../styles/app.css";
import Landing from "../components/Landing.svelte";

// Mounted into <body> rather than into a wrapper: the stylesheet addresses
// the bar as `body > nav`, and an element in between would silently stop every
// one of those rules from matching.
mount(Landing, { target: document.body });
