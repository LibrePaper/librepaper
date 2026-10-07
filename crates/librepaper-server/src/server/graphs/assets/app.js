// The graphs page: one uPlot per series the server returns, in the server's
// order. No framework. Colors come from CSS variables, and nothing here sets a
// style attribute, because the page's CSP forbids inline styles.
(function () {
  "use strict";

  const MAX_POINTS = 600;
  const MIN_POINTS = 10;
  const RECORD = 60; // seconds between records
  const HEIGHT = 190;
  const REFRESH_MS = 60 * 1000;
  const MIN_SPAN = RECORD; // a drag narrower than a record is widened
  const SIZES = ["B", "KB", "MB", "GB", "TB"];
  // The least a y axis may reach, so an idle series is not stretched to fill it.
  const FLOOR = { seconds: 0.005, percent: 1 };

  const grid = document.getElementById("grid");
  const bar = document.getElementById("ranges");
  const status = document.getElementById("status");
  const dark = window.matchMedia("(prefers-color-scheme: dark)");

  let choice = 86400; // seconds, from the range bar
  let zoom = null; // {from, to} after a drag-select, else null
  let panels = []; // one {title, plot} per series
  let latest = 0; // the newest request; a slower, older reply is dropped
  let pending = false; // a load is already scheduled for this turn

  const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

  function round(value, digits) {
    const scale = Math.pow(10, digits);
    return Math.round(value * scale) / scale;
  }

  // One number in the unit its series is measured in.
  function format(value, unit) {
    if (value == null) return "";
    if (unit === "bytes") {
      let size = Math.abs(value);
      let step = 0;
      while (size >= 1024 && step < SIZES.length - 1) {
        size /= 1024;
        step++;
      }
      return round(value / Math.pow(1024, step), 2) + " " + SIZES[step];
    }
    if (unit === "seconds") return round(value * 1000, 1) + " ms";
    if (unit === "percent") return round(value, 1) + "%";
    return String(round(value, 2));
  }

  // A max series is the worst minute in each cell once a cell spans several
  // minutes. The latency histogram ends at 5 s, so that value is a floor.
  function titleFor(series, step) {
    const notes = [];
    if (series.bucket === "max" && step > RECORD) notes.push("worst minute");
    if (series.name === "latency_p95_seconds") notes.push("5 s means at least 5 s");
    return series.label + (notes.length ? " (" + notes.join(", ") + ")" : "");
  }

  function options(series, width) {
    const rule = css("--rule");
    const axis = { stroke: css("--ink"), grid: { stroke: rule, width: 1 }, ticks: { stroke: rule, width: 1 } };
    const floor = FLOOR[series.unit] || 1;
    return {
      width: width,
      height: HEIGHT,
      cursor: {
        sync: { key: "graphs" },
        drag: { x: true, y: false, setScale: false },
        // Double-click goes back to the range bar's choice, not to uPlot's own reset.
        bind: { dblclick: () => () => reset() },
      },
      scales: { y: { range: (u, min, max) => [0, Math.max(Number.isFinite(max) ? max : 0, floor) * 1.1] } },
      series: [
        {},
        {
          label: "Value",
          stroke: css("--line"),
          fill: css("--fill"),
          width: 1.5,
          spanGaps: false,
          value: (u, v) => format(v, series.unit) || "no data",
        },
      ],
      axes: [axis, Object.assign({ size: 70, values: (u, ticks) => ticks.map((v) => format(v, series.unit)) }, axis)],
      hooks: { setSelect: [selected] },
    };
  }

  // Cursor sync delivers a drag, and a double-click, to every panel, so each
  // asks for a load and the turn's loads become one request.
  function schedule() {
    if (pending) return;
    pending = true;
    queueMicrotask(() => {
      pending = false;
      load();
    });
  }

  // A drag on any panel zooms every panel to that span.
  function selected(u) {
    if (u.select.width < 4) return;
    const from = Math.floor(u.posToVal(u.select.left, "x"));
    const to = Math.ceil(u.posToVal(u.select.left + u.select.width, "x"));
    u.setSelect({ left: 0, top: 0, width: 0, height: 0 }, false);
    zoom = { from: from, to: Math.max(to, from + MIN_SPAN) };
    schedule();
  }

  function reset() {
    zoom = null;
    schedule();
  }

  function makePanel(series, xs) {
    const box = document.createElement("section");
    box.className = "panel";
    const title = document.createElement("h2");
    const holder = document.createElement("div");
    holder.className = "plot";
    box.append(title, holder);
    grid.append(box);
    const plot = new uPlot(options(series, holder.clientWidth || 320), [xs, series.values], holder);
    // uPlot needs an explicit width and height, so the container is watched.
    new ResizeObserver(() => {
      const width = holder.clientWidth;
      if (width > 0 && width !== plot.width) plot.setSize({ width: width, height: HEIGHT });
    }).observe(holder);
    return { title: title, plot: plot };
  }

  function clear() {
    panels.forEach((panel) => panel.plot.destroy());
    panels = [];
    grid.replaceChildren();
  }

  function render(data) {
    const xs = data.series.length ? data.series[0].values.map((_, i) => data.from + i * data.step) : [];
    if (panels.length !== data.series.length) {
      clear();
      panels = data.series.map((series) => makePanel(series, xs));
    }
    data.series.forEach((series, i) => {
      panels[i].title.textContent = titleFor(series, data.step);
      panels[i].plot.setData([xs, series.values]);
    });
  }

  async function load() {
    const id = ++latest;
    const to = zoom ? zoom.to : Math.floor(Date.now() / 1000);
    const from = zoom ? zoom.from : to - choice;
    // A cell is never narrower than a record, or each sample would sit alone
    // between gaps and draw nothing.
    const points = Math.min(MAX_POINTS, Math.max(MIN_POINTS, Math.floor((to - from) / RECORD)));
    try {
      const response = await fetch("/data?from=" + from + "&to=" + to + "&points=" + points, { cache: "no-store" });
      if (!response.ok) throw new Error("HTTP " + response.status);
      const data = await response.json();
      if (id !== latest) return;
      render(data);
      status.classList.remove("failed");
      status.textContent = (zoom ? "Zoomed, double-click to reset. " : "") + "Updated " + new Date().toLocaleTimeString();
    } catch (error) {
      if (id !== latest) return;
      status.classList.add("failed");
      status.textContent = "Could not load: " + error.message;
    }
  }

  bar.addEventListener("click", (event) => {
    const button = event.target.closest("button");
    if (!button) return;
    bar.querySelectorAll("button").forEach((other) => other.classList.toggle("active", other === button));
    choice = Number(button.dataset.seconds);
    zoom = null;
    load();
  });

  // While the range ends at now, the page follows it.
  setInterval(() => {
    if (!zoom && !document.hidden) load();
  }, REFRESH_MS);

  // Canvas colors are read when a panel is made, so a scheme change rebuilds them.
  dark.addEventListener("change", () => {
    clear();
    load();
  });

  load();
})();
