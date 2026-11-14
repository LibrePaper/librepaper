// The house stylesheet's text, bundled with the app. It is the only copy:
// the wasm renderers no longer embed it, so every page that shows it takes it
// from here.
import css from "../styles/document.css?raw";

export default css;
