<script>
  // The icons the shell uses, from Lucide, drawn as shapes rather than fetched
  // as a font or a sprite: a few small paths cost less than either, and an
  // icon that is part of the bundle cannot arrive after the button it belongs
  // to. See styles/lucide-LICENSE.txt.
  //
  // The geometry is Lucide's own, rounded corners included. Redrawing a
  // rounded rectangle as a square-cornered path is what made these look like
  // icons from two different sets.
  const ICONS = {
    book: [["path", "M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H19a1 1 0 0 1 1 1v18a1 1 0 0 1-1 1H6.5a1 1 0 0 1 0-5H20"]],
    "panel-left": [["rect", { width: 18, height: 18, x: 3, y: 3, rx: 2 }], ["path", "M9 3v18"]],
    "panel-right": [["rect", { width: 18, height: 18, x: 3, y: 3, rx: 2 }], ["path", "M15 3v18"]],
    "columns-2": [["rect", { width: 18, height: 18, x: 3, y: 3, rx: 2 }], ["path", "M12 3v18"]],
    // Lucide's panel-left-open, for the column at the left of the window. Not
    // panel-left, which is the "source only" arrangement's and would say the
    // wrong thing beside it.
    "panel-left-open": [
      ["rect", { width: 18, height: 18, x: 3, y: 3, rx: 2 }],
      ["path", "M9 3v18"],
      ["path", "m14 9 3 3-3 3"],
    ],
    // Lucide's folder, image and download, for the files panel: the panel
    // itself, a figure to add to it, and the directory as a zip.
    folder: [
      ["path", "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"],
    ],
    image: [
      ["rect", { width: 18, height: 18, x: 3, y: 3, rx: 2, ry: 2 }],
      ["circle", { cx: 9, cy: 9, r: 2 }],
      ["path", "m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"],
    ],
    download: [
      ["path", "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"],
      ["path", "m7 10 5 5 5-5"],
      ["path", "M12 15V3"],
    ],
    "message-square": [["path", "M22 17a2 2 0 0 1-2 2H6l-4 4V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z"]],
    "file-text": [
      ["path", "M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"],
      ["path", "M14 2v4a2 2 0 0 0 2 2h4"],
      ["path", "M10 9H8"],
      ["path", "M16 13H8"],
      ["path", "M16 17H8"],
    ],
    // Lucide's pencil, for renaming a file in the directory.
    pencil: [
      ["path", "M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"],
      ["path", "m15 5 4 4"],
    ],
    // Lucide's file-plus, for adding one.
    "file-plus": [
      ["path", "M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"],
      ["path", "M14 2v4a2 2 0 0 0 2 2h4"],
      ["path", "M9 15h6"],
      ["path", "M12 18v-6"],
    ],
    lock: [["rect", { width: 18, height: 11, x: 3, y: 11, rx: 2, ry: 2 }], ["path", "M7 11V7a5 5 0 0 1 10 0v4"]],
    unlock: [["rect", { width: 18, height: 11, x: 3, y: 11, rx: 2, ry: 2 }], ["path", "M7 11V7a5 5 0 0 1 9.9-1"]],
    help: [
      ["circle", { cx: 12, cy: 12, r: 10 }],
      ["path", "M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"],
      ["path", "M12 17h.01"],
    ],
    save: [
      ["path", "M15.2 3a2 2 0 0 1 1.4.6l3.8 3.8a2 2 0 0 1 .6 1.4V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"],
      ["path", "M17 21v-7a1 1 0 0 0-1-1H8a1 1 0 0 0-1 1v7"],
      ["path", "M7 3v4a1 1 0 0 0 1 1h7"],
    ],
    link: [
      ["path", "M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"],
      ["path", "M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"],
    ],
    check: [["path", "M20 6 9 17l-5-5"]],
    // Lucide's history, for the timeline.
    history: [
      ["path", "M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"],
      ["path", "M3 3v5h5"],
      ["path", "M12 7v5l4 2"],
    ],
    comment: [
      ["path", "M22 17a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 21.286V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z"],
      ["path", "M7 11h10"],
      ["path", "M7 15h6"],
      ["path", "M7 7h8"],
    ],
    highlight: [
      ["path", "m9 11-6 6v3h9l3-3"],
      ["path", "m22 12-4.6 4.6a2 2 0 0 1-2.8 0l-5.2-5.2a2 2 0 0 1 0-2.8L14 4"],
    ],
    box: [
      ["path", "M5 3a2 2 0 0 0-2 2"], ["path", "M19 3a2 2 0 0 1 2 2"], ["path", "M5 21a2 2 0 0 1-2-2"],
      ["path", "M9 3h1"], ["path", "M9 21h2"], ["path", "M14 3h1"],
      ["path", "M3 9v1"], ["path", "M21 9v2"], ["path", "M3 14v1"],
      ["path", "m21 15-3 3-2-2-3 3v-8z"],
    ],
    star: [["path", "M11.5 2.5a.6.6 0 0 1 1 0l2.5 5.1 5.6.8a.6.6 0 0 1 .3 1l-4 4 1 5.6a.6.6 0 0 1-.9.6L12 17l-5 2.6a.6.6 0 0 1-.9-.6l1-5.6-4-4a.6.6 0 0 1 .3-1l5.6-.8z"]],
    upload: [
      ["path", "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"],
      ["path", "m17 8-5-5-5 5"],
      ["path", "M12 3v12"],
    ],
    search: [["circle", { cx: 11, cy: 11, r: 8 }], ["path", "m21 21-4.3-4.3"]],
    // Sharing is about people rather than about a URL, so the control that
    // opens the dialog wears people; copying the link is a `link` inside it.
    users: [
      ["path", "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"],
      ["circle", { cx: 9, cy: 7, r: 4 }],
      ["path", "M22 21v-2a4 4 0 0 0-3-3.87"],
      ["path", "M16 3.13a4 4 0 0 1 0 7.75"],
    ],
    trash: [
      ["path", "M3 6h18"],
      ["path", "M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"],
      ["path", "M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"],
    ],
  };

  // Skeleton's button sizes the icon inside it, so the default here is only
  // for an icon that stands on its own.
  //
  // A few of these say whether something is on -- a star is either a favourite
  // or it is not -- and an outline that only changes colour reads as a click
  // that did not register. Those are drawn solid instead.
  let { name, size = null, filled = false } = $props();
</script>

<svg
  viewBox="0 0 24 24"
  width={size}
  height={size}
  fill={filled ? "currentColor" : "none"}
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#each ICONS[name] ?? [] as [shape, geometry]}
    {#if shape === "rect"}
      <rect {...geometry} />
    {:else if shape === "circle"}
      <circle {...geometry} />
    {:else}
      <path d={geometry} />
    {/if}
  {/each}
</svg>

<style>
  /* An inline SVG sits on the text baseline, which leaves a few pixels of
     descender space under it and pushes it off centre in a button that is
     otherwise square. */
  svg {
    display: block;
    flex: none;
  }
</style>
