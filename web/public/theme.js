// Sets the mode and the colour theme before the bundle runs, so a dark or
// coloured page never flashes light. It repeats the resolution in
// src/lib/theme.js on purpose: a classic script cannot import, and the server
// allows no inline script.
(function () {
  var mode = "light";
  var palette = "librepaper";
  try {
    var choice = JSON.parse(localStorage.getItem("librepaper-theme"));
    if (choice === "dark" || choice === "light") mode = choice;
    else if (matchMedia("(prefers-color-scheme: dark)").matches) mode = "dark";
    var stored = JSON.parse(localStorage.getItem("librepaper-palette"));
    if (["librepaper", "flexoki", "solarized", "gruvbox", "catppuccin", "nord"].indexOf(stored) !== -1) palette = stored;
  } catch (error) {
    /* storage or matchMedia is unavailable: light, and the default palette */
  }
  document.documentElement.dataset.mode = mode;
  document.documentElement.dataset.palette = palette;
})();
