// The landing page entry.
//
// The page's words are in index.html, not here: this is the page a crawler
// indexes and a stranger is sent, so its text has to be in the document that
// arrives rather than in a bundle that has to run first. What is left for
// JavaScript is the bar, which is furniture -- the page reads correctly
// before it mounts, and would read correctly if it never did.
import { mount } from "svelte";
import "./site.css";
import SiteBar from "./SiteBar.svelte";

mount(SiteBar, { target: document.getElementById("siteBar") });
