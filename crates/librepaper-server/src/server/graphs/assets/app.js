// The admin page: resources graphs, the accounts list and moderation. No
// framework. Colors come from CSS variables, and nothing here sets a style
// attribute, because the page's CSP forbids inline styles.
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
  const TABS = ["resources", "accounts", "moderation"];

  const grid = document.getElementById("grid");
  const bar = document.getElementById("ranges");
  const status = document.getElementById("status");
  const tabs = document.getElementById("tabs");
  const dark = window.matchMedia("(prefers-color-scheme: dark)");

  let choice = 86400; // seconds, from the range bar
  let zoom = null; // {from, to} after a drag-select, else null
  let panels = []; // one {title, plot} per series
  let latest = 0; // the newest request; a slower, older reply is dropped
  let pending = false; // a load is already scheduled for this turn
  let tab = "resources"; // the tab on show
  let focusReason = false; // focus the block reason once moderation is shown

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

  // The indices of samples with a gap, or an edge, on both sides. A line needs
  // two neighbours to be seen, so a lone sample is drawn as a mark instead; a
  // run of samples stays a line. An idle instance has a p95 only in the minutes
  // that had requests, so most of its samples are lone.
  function isolated(u, seriesIdx) {
    const ys = u.data[seriesIdx];
    const lone = [];
    for (let i = 0; i < ys.length; i++) {
      const gapBefore = i === 0 || ys[i - 1] == null;
      const gapAfter = i === ys.length - 1 || ys[i + 1] == null;
      if (ys[i] != null && gapBefore && gapAfter) lone.push(i);
    }
    return lone;
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
          points: { show: true, size: 5, fill: css("--line"), filter: isolated },
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
    if (!zoom && !document.hidden && tab === "resources") load();
  }, REFRESH_MS);

  // Canvas colors are read when a panel is made, so a scheme change rebuilds them.
  dark.addEventListener("change", () => {
    clear();
    if (tab === "resources") load();
  });

  // The fifty accounts holding the most storage. Handles are chosen by users,
  // so every cell is filled with textContent, never markup.
  function day(seconds) {
    return new Date(seconds * 1000).toISOString().slice(0, 10);
  }

  function cell(text, className) {
    const td = document.createElement("td");
    if (className) td.className = className;
    td.textContent = text;
    return td;
  }

  // Opens the moderation tab with this account's id and provider filled in.
  function blockAccount(account) {
    document.getElementById("block-target").value = account.id;
    if (account.provider === "github" || account.provider === "google") {
      document.getElementById("block-provider").value = account.provider;
    }
    focusReason = true;
    if (location.hash === "#moderation") show("moderation");
    else location.hash = "#moderation";
  }

  // `dueBefore` is the last sign-in, in seconds, below which an account is due for erasure within 30 days, or null when the sweep is off.
  function accountRow(account, dueBefore) {
    const row = document.createElement("tr");
    const name = document.createElement("td");
    name.append(account.handle);
    if (account.provider) {
      const provider = document.createElement("span");
      provider.className = "muted";
      provider.textContent = account.provider;
      name.append(" ", provider);
    }
    const lastSeen = cell(day(account.last_seen_at));
    if (dueBefore != null && account.last_seen_at < dueBefore) lastSeen.className = "due";
    const id = document.createElement("td");
    const code = document.createElement("code");
    code.textContent = account.id;
    id.append(code);
    const action = document.createElement("td");
    if (account.status === "active") {
      const block = document.createElement("button");
      block.type = "button";
      block.className = "danger";
      block.textContent = "Block";
      block.addEventListener("click", () => blockAccount(account));
      action.append(block);
    }
    row.append(
      name,
      cell(account.status),
      cell(format(account.bytes, "bytes"), "num"),
      cell(String(account.documents), "num"),
      lastSeen,
      cell(day(account.created_at)),
      id,
      action
    );
    return row;
  }

  async function loadAccounts() {
    const total = document.getElementById("accounts-total");
    const expiry = document.getElementById("accounts-expiry");
    const rows = document.getElementById("accounts-rows");
    try {
      const response = await fetch("/accounts", { cache: "no-store" });
      if (!response.ok) throw new Error("HTTP " + response.status);
      const data = await response.json();
      total.textContent = "Stored: " + format(data.total_bytes, "bytes") + " of " + format(data.limit_bytes, "bytes");
      let dueBefore = null;
      if (data.erase_inactive_after > 0) {
        const days = Math.round(data.erase_inactive_after / 86400);
        const soon = data.erasing_soon;
        expiry.textContent = "Accounts with no sign-in for " + days + " days are erased. Within 30 days: " +
          soon.accounts + " accounts holding " + format(soon.bytes, "bytes") + ".";
        dueBefore = Date.now() / 1000 - data.erase_inactive_after + 30 * 86400;
      } else {
        expiry.textContent = "Accounts with no sign-in are kept.";
      }
      rows.replaceChildren(...data.accounts.map((account) => accountRow(account, dueBefore)));
    } catch (error) {
      total.classList.add("failed");
      total.textContent = "The accounts could not be loaded.";
    }
  }

  // The moderation lists and their actions. This reads and changes the
  // moderation lists. Every name comes from users, so it only ever goes in as
  // text.
  const RECENT_ACTIONS = {
    block_account: "Blocked",
    unblock_account: "Unblocked",
    hide_project: "Hid",
    unhide_project: "Unhid",
    ban_handle: "Refused name",
    unban_handle: "Removed refusal",
  };
  const OUTCOMES = {
    blocked: (label) => "Blocked " + label + ".",
    banned: (label) => "Refused " + label + " at its first sign-in.",
    unblocked: (label) => "Unblocked " + label + ".",
    unbanned: (label) => "Removed the refusal of " + label + ".",
    hidden: (label) => "Hid " + label + ".",
    unhidden: (label) => "Unhid " + label + ".",
  };
  const LOAD_FAILED = "The moderation lists could not be loaded.";

  // A date in whole days, or an empty cell when the server has none.
  function when(seconds) {
    return seconds == null ? "" : day(seconds);
  }

  // A cell with a muted second line, such as a provider or a project title.
  function labelled(text, extra) {
    const td = document.createElement("td");
    td.append(text || "");
    if (extra) {
      const span = document.createElement("span");
      span.className = "muted";
      span.textContent = extra;
      td.append(" ", span);
    }
    return td;
  }

  // Fills a table body, or shows one "None." row across all its columns.
  function fill(body, rows, columns, build) {
    if (!rows.length) {
      const td = cell("None.", "muted");
      td.colSpan = columns;
      const row = document.createElement("tr");
      row.append(td);
      body.replaceChildren(row);
      return;
    }
    body.replaceChildren(...rows.map(build));
  }

  // The last cell of an undo row. Its button opens a reason form in place.
  function undoCell(label, action, target) {
    const td = document.createElement("td");
    const showButton = () => {
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = label;
      button.addEventListener("click", showForm);
      td.replaceChildren(button);
    };
    const showForm = () => {
      const form = document.createElement("form");
      const reason = document.createElement("input");
      reason.className = "reason";
      reason.placeholder = "Reason";
      reason.required = true;
      reason.maxLength = 2000;
      const confirm = document.createElement("button");
      confirm.type = "submit";
      confirm.textContent = "Confirm";
      const cancel = document.createElement("button");
      cancel.type = "button";
      cancel.textContent = "Cancel";
      cancel.addEventListener("click", showButton);
      form.append(reason, confirm, cancel);
      form.addEventListener("submit", async (event) => {
        event.preventDefault();
        const ok = await act({ action: action, target: target, provider: "", reason: reason.value }, confirm);
        if (ok) loadModeration();
      });
      td.replaceChildren(form);
      reason.focus();
    };
    showButton();
    return td;
  }

  // Sends one POST. A failure carries the server's message, or the text, or the status.
  async function post(request) {
    const response = await fetch("/moderation", {
      method: "POST",
      cache: "no-store",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    });
    const body = await response.text();
    if (!response.ok) {
      let message = "";
      try {
        message = JSON.parse(body).error;
      } catch (error) {
        message = "";
      }
      throw new Error(message || body || "HTTP " + response.status);
    }
    return JSON.parse(body);
  }

  // Runs one action with its submit button disabled. Returns whether it worked.
  async function act(request, submit) {
    const note = document.getElementById("moderation-status");
    submit.disabled = true;
    try {
      const result = await post(request);
      const say = OUTCOMES[result.outcome];
      note.classList.remove("failed");
      note.textContent = say ? say(result.label) : "Done.";
      return true;
    } catch (error) {
      note.classList.add("failed");
      note.textContent = error.message;
      return false;
    } finally {
      submit.disabled = false;
    }
  }

  async function loadModeration() {
    const note = document.getElementById("moderation-status");
    try {
      const response = await fetch("/moderation", { cache: "no-store" });
      if (!response.ok) throw new Error("HTTP " + response.status);
      const data = await response.json();
      fill(document.getElementById("blocked-rows"), data.blocked_accounts, 5, (account) => {
        const row = document.createElement("tr");
        row.append(
          labelled(account.handle, account.provider),
          cell(when(account.blocked_at)),
          cell(account.actor || ""),
          cell(account.reason || "", "reason"),
          undoCell("Unblock", "unblock", account.id)
        );
        return row;
      });
      fill(document.getElementById("banned-rows"), data.banned_handles, 5, (entry) => {
        const row = document.createElement("tr");
        row.append(
          labelled(entry.handle, entry.provider),
          cell(when(entry.created_at)),
          cell(entry.actor || ""),
          cell(entry.reason || "", "reason"),
          undoCell("Remove", "unban", entry.id)
        );
        return row;
      });
      fill(document.getElementById("hidden-rows"), data.hidden_projects, 6, (project) => {
        const row = document.createElement("tr");
        row.append(
          labelled(project.slug, project.title),
          cell(project.owner || ""),
          cell(when(project.hidden_at)),
          cell(project.actor || ""),
          cell(project.reason || "", "reason"),
          undoCell("Unhide", "unhide", project.slug)
        );
        return row;
      });
      fill(document.getElementById("recent-rows"), data.recent, 5, (entry) => {
        const row = document.createElement("tr");
        row.append(
          cell(new Date(entry.at * 1000).toISOString().slice(0, 16).replace("T", " ")),
          cell(entry.actor || ""),
          cell(RECENT_ACTIONS[entry.action] || entry.action),
          cell(entry.target || ""),
          cell(entry.reason || "", "reason")
        );
        return row;
      });
      if (note.textContent === LOAD_FAILED) {
        note.classList.remove("failed");
        note.textContent = "";
      }
    } catch (error) {
      note.classList.add("failed");
      note.textContent = LOAD_FAILED;
    }
  }

  document.getElementById("block-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.currentTarget;
    const ok = await act(
      {
        action: "block",
        target: document.getElementById("block-target").value,
        provider: document.getElementById("block-provider").value,
        reason: document.getElementById("block-reason").value,
      },
      form.querySelector("button[type=submit]")
    );
    if (ok) {
      form.reset();
      loadModeration();
    }
  });

  document.getElementById("hide-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.currentTarget;
    const ok = await act(
      {
        action: "hide",
        target: document.getElementById("hide-target").value,
        provider: "",
        reason: document.getElementById("hide-reason").value,
      },
      form.querySelector("button[type=submit]")
    );
    if (ok) {
      form.reset();
      loadModeration();
    }
  });

  // The tab named by the hash. Anything else is the resources tab.
  function tabFromHash() {
    const name = location.hash.slice(1);
    return name === "accounts" || name === "moderation" ? name : "resources";
  }

  // Shows one tab and loads its data each time it is shown.
  function show(name) {
    tab = name;
    tabs.querySelectorAll("button").forEach((button) => {
      const on = button.dataset.tab === name;
      button.classList.toggle("active", on);
      button.setAttribute("aria-selected", on ? "true" : "false");
    });
    TABS.forEach((other) => {
      document.getElementById("tab-" + other).hidden = other !== name;
    });
    if (name === "resources") load();
    else if (name === "accounts") loadAccounts();
    else loadModeration();
    if (name === "moderation" && focusReason) {
      focusReason = false;
      document.getElementById("block-reason").focus();
    }
  }

  tabs.addEventListener("click", (event) => {
    const button = event.target.closest("button");
    if (!button) return;
    if (button.dataset.tab === tab) show(tab);
    else location.hash = button.dataset.tab;
  });

  window.addEventListener("hashchange", () => show(tabFromHash()));

  show(tabFromHash());
})();
