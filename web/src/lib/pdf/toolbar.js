import { GrabToPan } from "./vendor/grab_to_pan.js";
import zoomIn from "./icons/zoom-in.svg";
import zoomOut from "./icons/zoom-out.svg";
import selectTool from "./icons/text-cursor.svg";
import handTool from "./icons/hand.svg";

// PDF.js 5.4.149 controls with Lucide icons, adapted to our annotated text renderer.
// A shadow root keeps control labels out of the agent's document text offsets.
export function createToolbar(onScale) {
  const element = document.createElement("div");
  element.className = "pdf-toolbar";
  const root = element.attachShadow({ mode: "open" });
  root.innerHTML = `<style>
    :host { color-scheme: light dark; }
    .toolbar { display:flex; align-items:center; justify-content:center; gap:4px; height:40px;
      box-sizing:border-box; padding:4px; background:ButtonFace;
      border-bottom:1px solid ButtonBorder; font:12px system-ui; }
    button { width:28px; height:28px; border:0; border-radius:4px; background:transparent; }
    button:hover, button[aria-pressed="true"] { background:SelectedItem; }
    button:disabled { opacity:.4; }
    button::before { content:""; display:block; width:16px; height:16px; margin:auto;
      background:ButtonText; mask:var(--icon) center / contain no-repeat; }
    select { height:28px; min-width:120px; font:inherit; }
    .tools { display:flex; border-left:1px solid ButtonBorder; margin-left:4px; padding-left:4px; }
  </style><div class="toolbar" role="toolbar" aria-label="PDF controls">
    <button id="zoomOutButton" title="Zoom Out" aria-label="Zoom Out"></button>
    <button id="zoomInButton" title="Zoom In" aria-label="Zoom In"></button>
    <select id="scaleSelect" aria-label="Zoom">
      <option value="auto">Automatic Zoom</option>
      <option value="page-actual">Actual Size</option>
      <option value="page-fit">Page Fit</option>
      <option value="page-width">Page Width</option>
      <option value="custom" disabled hidden></option>
      ${[50,75,100,125,150,200,300,400].map(n => `<option value="${n/100}">${n}%</option>`).join("")}
    </select>
    <div class="tools" role="group" aria-label="Cursor tools">
      <button id="cursorSelectTool" title="Text Selection Tool" aria-label="Text Selection Tool" aria-pressed="true"></button>
      <button id="cursorHandTool" title="Hand Tool" aria-label="Hand Tool" aria-pressed="false"></button>
    </div>
  </div>`;
  const get = id => root.getElementById(id);
  for (const [id, icon] of Object.entries({ zoomInButton: zoomIn, zoomOutButton: zoomOut,
    cursorSelectTool: selectTool, cursorHandTool: handTool })) {
    get(id).style.setProperty("--icon", `url("${icon}")`);
  }
  const hand = new GrabToPan({ element: document.documentElement });
  const ignore = hand.ignoreTarget.bind(hand);
  hand.ignoreTarget = node => node === element || ignore(node);
  let scale = 1;
  const change = value => { onScale(String(value)); };
  get("scaleSelect").onchange = event => change(event.target.value);
  get("zoomInButton").onclick = () => change(scale = Math.min(10, Math.ceil(Number((scale * 1.1).toFixed(2)) * 10) / 10));
  get("zoomOutButton").onclick = () => change(scale = Math.max(0.1, Math.floor(Number((scale / 1.1).toFixed(2)) * 10) / 10));
  for (const [id, active] of [["cursorSelectTool", false], ["cursorHandTool", true]]) {
    get(id).onclick = () => {
      active ? hand.activate() : hand.deactivate();
      if (active) getSelection()?.removeAllRanges();
      get("cursorSelectTool").setAttribute("aria-pressed", String(!active));
      get("cursorHandTool").setAttribute("aria-pressed", String(active));
    };
  }
  return {
    element,
    destroy: () => hand.deactivate(),
    update(mode, currentScale) {
      scale = currentScale;
      const select = get("scaleSelect");
      select.value = mode;
      if (!select.value) {
        const custom = select.querySelector('[value="custom"]');
        custom.textContent = `${Math.round(scale * 100)}%`;
        select.value = "custom";
      }
      get("zoomOutButton").disabled = scale <= 0.1;
      get("zoomInButton").disabled = scale >= 10;
    },
  };
}
