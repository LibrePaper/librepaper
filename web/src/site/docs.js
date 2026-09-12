// The docs pages' entry, shared by every page web/tools/build-site.mjs
// writes into site/.build. Mirrors web/src/entries/documentation.js -- the
// image lightbox and the scrollspy on the page's own table of contents --
// minus the parts that only make sense inside the application: there is no
// whoami here, so the bar is SiteBar rather than Nav, and no history to read
// a heading position back out of.
import { mount } from "svelte";
import "./site.css";
import SiteBar from "./SiteBar.svelte";

mount(SiteBar, { target: document.getElementById("siteBar") });

/* ----------------------------------------------------------------- drawer */

// The drawer opens and closes on its own -- it is a <details> -- so this only
// adds the two things a reader expects of something that covers the page and
// that the element does not do by itself: Escape shuts it, and so does
// reaching past it.
const drawer = document.querySelector(".sitenav-drawer");
if (drawer) {
  // The markup ships open so that a reader with no script keeps the
  // navigation at every width. With a script, the drawer is a drawer only
  // where there is no room for a column: closed below the breakpoint, open
  // above it, and kept in step when the window crosses it -- so a drawer
  // somebody shut on a phone does not follow them into a wide window and
  // leave the column empty.
  const wide = matchMedia("(min-width: 900px)");
  const fitLayout = () => (drawer.open = wide.matches);
  fitLayout();
  wide.addEventListener("change", fitLayout);

  // The two things a reader expects of something covering the page that a
  // <details> does not do by itself.
  addEventListener("keydown", (event) => {
    if (event.key === "Escape" && drawer.open && !wide.matches) {
      drawer.open = false;
      drawer.querySelector("summary")?.focus();
    }
  });
  addEventListener("click", (event) => {
    if (drawer.open && !wide.matches && !drawer.contains(event.target)) drawer.open = false;
  });
}

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
  const links = new Map([...toc.querySelectorAll("a")].map((a) => [a.hash.slice(1), a]));

  // Which section is being read, rather than which heading last crossed a
  // line: the one nearest above the top of the window, so a long section
  // stays marked all the way down it instead of the mark falling off the end
  // and leaving nothing lit. The last heading stays current at the foot of
  // the page, where nothing further can scroll into view.
  let current = "";
  function follow() {
    const top = 120;
    let found = headings[0]?.id ?? "";
    for (const heading of headings) {
      if (heading.getBoundingClientRect().top <= top) found = heading.id;
      else break;
    }
    if (found === current) return;
    current = found;
    for (const [id, link] of links) link.toggleAttribute("aria-current", id === found);
    // Keep the marked entry inside a contents list long enough to scroll.
    links.get(found)?.scrollIntoView({ block: "nearest" });
  }

  // Cheap enough to run on every frame the browser was going to paint anyway,
  // and a listener that reads layout on every scroll event is not.
  let queued = false;
  const schedule = () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      follow();
    });
  };
  addEventListener("scroll", schedule, { passive: true });
  addEventListener("resize", schedule, { passive: true });
  follow();
}
