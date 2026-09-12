// The docs pages' entry, shared by every page web/tools/build-site.mjs
// writes into site/.build. Mirrors web/src/entries/documentation.js -- the
// image lightbox and the scrollspy on the page's own table of contents --
// minus the parts that only make sense inside the application: there is no
// whoami here, so the bar is SiteBar rather than Nav, and no history to read
// a heading position back out of.
import { mount } from "svelte";
import "./site.css";
import Hero from "../components/Hero.svelte";
import SiteBar from "./SiteBar.svelte";

mount(SiteBar, { target: document.getElementById("siteBar") });
mount(Hero, { target: document.getElementById("hero") });

/* ---------------------------------------------------------------- images */

const lightbox = document.getElementById("imageLightbox");
const expanded = lightbox?.querySelector("img");

if (lightbox && expanded) {
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
}

/* ---------------------------------------------------------------- contents */

const toc = document.getElementById("tableOfContents");
if (toc && toc.querySelector("a")) {
  const headings = [...document.querySelectorAll(".prose h2, .prose h3")];

  // Beside the text it is a list that is simply there; above the text, on a
  // narrow screen, an open one would bury the document under its own
  // headings, so it starts closed and follows the layout it is in.
  const wide = matchMedia("(min-width: 1100px)");
  const details = toc.closest("details");
  const fitLayout = () => (details.open = wide.matches);
  fitLayout();
  wide.addEventListener("change", fitLayout);
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
}
