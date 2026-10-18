// Sets the mode before the bundle runs, so a dark page never flashes light.
// It repeats the resolution in src/lib/theme.js on purpose: a classic script
// cannot import, and the server allows no inline script.
(function () {
  var mode = "light";
  try {
    var choice = JSON.parse(localStorage.getItem("librepaper-theme"));
    if (choice === "dark" || choice === "light") mode = choice;
    else if (matchMedia("(prefers-color-scheme: dark)").matches) mode = "dark";
  } catch (error) {
    /* storage or matchMedia is unavailable: light */
  }
  document.documentElement.dataset.mode = mode;
})();
