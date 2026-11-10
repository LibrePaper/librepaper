// The widgets of Org-only tokens: the title block, keywords, TODO states, priorities,
// tags, timestamps, drawers, targets, macros and internal links. Org reuses the
// Markdown widgets where the meaning matches. index.js imports this file, so nothing
// here reads an index.js export at load time; the factories and toDOM are where they are used.
import { WidgetType } from "@codemirror/view";
import { chipWidget, ElementWidget, factsOf } from "./index.js";
import { registerWidget, revealOnMousedown } from "./markup.ts";
import { locale } from "../locale.js";
import { m } from "../paraglide/messages.js";
import "./org.css";

// A DOM element with a class and text, in one call.
function el(tag, className = "", text = "") {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text) element.textContent = text;
  return element;
}

// ---- Drawing ---------------------------------------------------------------

function drawHeader(data) {
  const block = el("div", "cm-lp-title cm-lp-org-header");
  if (data.title) block.append(el("div", "cm-lp-title-main", data.title));
  else block.append(el("div", "cm-lp-title-main cm-lp-title-muted", m.common_untitled()));
  if (data.subtitle) block.append(el("div", "cm-lp-title-affil", data.subtitle));
  if (data.author) block.append(el("div", "cm-lp-title-author", data.author));
  if (data.date) block.append(el("div", "cm-lp-title-date", data.date));
  return block;
}

function drawKeyword(data) {
  const chip = el("span", "cm-lp-org-keyword");
  chip.append(el("span", "cm-lp-org-keyword-key", `${data.key.toLowerCase()}:`));
  if (data.value) chip.append(" ", el("span", "cm-lp-org-keyword-value", data.value));
  return chip;
}

function drawTodo(data) {
  return el("span", `cm-lp-org-todo ${data.done ? "cm-lp-org-todo-done" : "cm-lp-org-todo-open"}`, data.state);
}

function drawPriority(data) {
  return el("span", "cm-lp-org-priority", data.value);
}

function drawTags(data) {
  const wrap = el("span", "cm-lp-org-tags");
  for (const tag of data.tags) wrap.append(el("span", "cm-lp-org-tag", tag));
  return wrap;
}

// The date of a timestamp such as "2024-03-05 Tue 10:00 +1w", in the interface language,
// followed by whatever else the stamp holds (the time, a repeater). Raw text when it does not parse.
function stampText(raw) {
  const match = /^(\d{4})-(\d{2})-(\d{2})(?:\s+[^\s\d+\-.][^\s]*)?(?:\s+(.*))?$/.exec(raw.trim());
  if (!match) return raw;
  const date = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]));
  if (Number.isNaN(date.getTime())) return raw;
  let day;
  try {
    day = new Intl.DateTimeFormat(locale(), { dateStyle: "medium" }).format(date);
  } catch {
    return raw;
  }
  return match[4] ? `${day} ${match[4]}` : day;
}

function drawStamp(data) {
  const chip = el("span", `cm-lp-org-stamp ${data.active ? "cm-lp-org-stamp-active" : "cm-lp-org-stamp-inactive"}`, stampText(data.text));
  chip.title = data.text;
  return chip;
}

function drawTarget(data) {
  const mark = el("span", "cm-lp-org-target", "⚓");
  mark.title = data.key;
  return mark;
}

function drawMacro(data) {
  const chip = el("span", "cm-lp-org-macro", data.name);
  if (data.args) chip.append(el("span", "cm-lp-org-macro-args", `(${data.args})`));
  return chip;
}

const DRAW = {
  header: drawHeader,
  keyword: drawKeyword,
  todo: drawTodo,
  priority: drawPriority,
  tags: drawTags,
  stamp: drawStamp,
  target: drawTarget,
  macro: drawMacro,
};

class OrgWidget extends WidgetType {
  constructor(name, data) {
    super();
    this.name = name;
    this.data = data;
    this.signature = JSON.stringify([name, data]);
  }

  eq(other) {
    return other instanceof OrgWidget && other.signature === this.signature;
  }

  toDOM(view) {
    const element = DRAW[this.name](this.data);
    revealOnMousedown(view, element);
    return element;
  }

  ignoreEvent() {
    return true;
  }
}

// ---- Registration ----------------------------------------------------------

registerWidget("orgheader", ({ token }) => {
  const meta = token.meta ?? {};
  return new OrgWidget("header", {
    title: meta.title ?? "",
    subtitle: meta.subtitle ?? "",
    author: meta.author ?? "",
    date: meta.date ?? "",
  });
});

registerWidget("orgkeyword", ({ token }) => {
  return new OrgWidget("keyword", { key: token.meta?.key ?? "", value: token.meta?.value ?? "" });
});

registerWidget("orgtodo", ({ token }) => {
  return new OrgWidget("todo", { state: token.meta?.state ?? "", done: token.meta?.done === "true" });
});

registerWidget("orgpriority", ({ token }) => new OrgWidget("priority", { value: token.meta?.value ?? "" }));

registerWidget("orgtags", ({ token }) => {
  const tags = (token.meta?.tags ?? "").split(",").map((tag) => tag.trim()).filter(Boolean);
  return new OrgWidget("tags", { tags });
});

registerWidget("orgstamp", ({ token }) => {
  return new OrgWidget("stamp", { text: token.meta?.text ?? "", active: token.meta?.active === "true" });
});

// A drawer, collapsed to a chip on a block of its own: "Properties (3)".
registerWidget("orgdrawer", ({ token }) => {
  const name = token.meta?.name ?? "";
  const count = Number(token.meta?.count) || 0;
  if (name.toUpperCase() === "PROPERTIES") return chipWidget(m.org_widgets_properties({ count }), true);
  const shown = name ? name.charAt(0).toUpperCase() + name.slice(1).toLowerCase() : name;
  return chipWidget(m.org_widgets_drawer({ name: shown, count }), true);
});

registerWidget("orgtarget", ({ token }) => new OrgWidget("target", { key: token.meta?.key ?? "" }));

registerWidget("orgmacro", ({ token }) => {
  return new OrgWidget("macro", { name: token.meta?.name ?? "", args: token.meta?.args ?? "" });
});

// An internal link. Org exports a link to a numbered element as its number.
registerWidget("orgref", ({ token, state }) => {
  const key = token.meta?.key ?? "";
  const description = token.meta?.description ?? "";
  const found = factsOf(state).labels.get(key);
  let content;
  if (description) content = [description];
  else if (found?.number) content = [String(found.number)];
  else if (found?.title) content = [found.title];
  else content = [{ text: `${key}?`, muted: true }];
  return new ElementWidget({ className: "cm-lp-ref cm-lp-org-ref", content, title: key });
});
