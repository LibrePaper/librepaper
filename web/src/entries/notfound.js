import { mount } from "svelte";
import "../styles/app.css";
import { start as startTheme } from "../lib/theme.js";
import NotFound from "../components/NotFound.svelte";
import { markDocumentLanguage } from "../lib/locale.js";
import { installTips } from "../lib/tip.js";

markDocumentLanguage();
// The one tooltip, for every element that carries data-tip.
installTips();

startTheme();

// Mounted into <body> rather than into a wrapper: the stylesheet addresses
// the bar as `body > nav`, and an element in between would silently stop every
// one of those rules from matching.
mount(NotFound, { target: document.body });
