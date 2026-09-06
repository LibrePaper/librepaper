// The documentation page: the project's README, with the shell's navigation
// around it and a contents list built from its own headings.
//
// The prose is not part of this bundle. It is put into the page when the page
// is served, from the README the binary embeds, so there is only ever one copy
// of that text and it cannot drift from the repository.
import { mount } from "svelte";
import "../styles/app.css";
import Nav from "../components/Nav.svelte";
import Hero from "../components/Hero.svelte";
import { me as whoami } from "../lib/api.js";

mount(Hero, { target: document.getElementById("hero") });
// The bar goes before the page's own markup, so it is the body's first child
// and the stylesheet's `body > nav` rules apply to it as they do everywhere
// else. Who you are is waited for rather than filled in afterwards: the prose
// is already on the screen, and a bar that changes shape a moment after it
// appears is worse than one that arrives complete.
mount(Nav, {
  target: document.body,
  anchor: document.querySelector("main"),
  props: { me: await whoami() },
});

/* ---------------------------------------------------------------- images */

// A screenshot is worth looking at properly, so clicking one opens it.
const lightbox = document.getElementById("imageLightbox");
const expanded = lightbox.querySelector("img");

for (const thumbnail of document.querySelectorAll(".prose img")) {
  thumbnail.tabIndex = 0;
  thumbnail.setAttribute("role", "button");
  thumbnail.setAttribute("aria-label", `Expand image: ${thumbnail.alt || "documentation image"}`);
  const open = () => {
    expanded.src = thumbnail.src;
    expanded.alt = thumbnail.alt;
    lightbox.showModal();
  };
  thumbnail.addEventListener("click", open);
  thumbnail.addEventListener("keydown", (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      open();
    }
  });
}
lightbox.querySelector("button").addEventListener("click", () => lightbox.close());
lightbox.addEventListener("click", (event) => {
  if (event.target === lightbox) lightbox.close();
});

/* ---------------------------------------------------------------- contents */

const toc = document.getElementById("tableOfContents");
const headings = [...document.querySelectorAll(".prose h2, .prose h3")];
for (const heading of headings) {
  const link = document.createElement("a");
  link.href = `#${heading.id}`;
  link.textContent = heading.textContent;
  link.dataset.level = heading.tagName.slice(1);
  toc.appendChild(link);
}
if (!headings.length) toc.closest(".toc").hidden = true;

// Beside the text it is a list that is simply there; above the text, on a
// narrow screen, an open one would bury the document under its own headings,
// so it starts closed and follows the layout it is in.
const wide = matchMedia("(min-width: 1100px)");
const details = toc.closest("details");
const fitLayout = () => (details.open = wide.matches);
fitLayout();
wide.addEventListener("change", fitLayout);
// Tapping a heading in the narrow layout should leave the list closed behind
// you, at the section you asked for.
toc.addEventListener("click", () => {
  if (!wide.matches) details.open = false;
});

const activate = (id) => {
  for (const link of toc.querySelectorAll("a")) link.toggleAttribute("aria-current", link.hash === `#${id}`);
};
const observer = new IntersectionObserver(
  (entries) => entries.forEach((entry) => entry.isIntersecting && activate(entry.target.id)),
  { rootMargin: "-15% 0px -70% 0px", threshold: 0 },
);
for (const heading of headings) observer.observe(heading);
