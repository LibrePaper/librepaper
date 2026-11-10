// Ranges of Org source that are markup rather than prose, for the spellchecker
// to skip. They come from the parse tree of our own Org grammar, so the
// editor, the document index and the spellchecker agree on what is a keyword,
// a drawer, a block, a link or a timestamp. One walk over the tree, so the
// scan is linear.
import { orgTree } from "../source-trees.js";

// Nodes that are markup from end to end.
const WHOLE = new Set([
  "Stars", "TodoKeyword", "Priority", "Tags",
  "Planning", "Clock", "TimestampActive", "TimestampInactive",
  "PropertyDrawer", "Drawer",
  "BlockBegin", "BlockEnd", "VerbatimBody", "CommentBody",
  "Comment", "FixedWidth", "HorizontalRule",
  "TableDelimiter", "TableRule", "TableFormula",
  "ListBullet", "Checkbox",
  "EmphasisMark", "Verbatim", "Code",
  "FootnoteLabel", "Citation", "Macro", "Target", "RadioTarget",
  "InlineMath", "DisplayMath", "LatexCommand", "Entity",
  "InlineSrc", "InlineBabelCall", "LatexEnvironment",
]);

// Keyword values that are prose. Every other #+KEY: line is markup.
const PROSE_KEYWORDS = new Set(["TITLE", "SUBTITLE", "CAPTION", "DESCRIPTION"]);

// Blocks whose bodies are code or raw text. An unclosed begin line of one of
// these is not a block in the tree, so its paragraph is skipped instead.
const SKIP_BLOCKS = new Set(["SRC", "EXAMPLE", "EXPORT", "COMMENT"]);
const BEGIN_LINE = /^#\+begin_(\w+)/i;

/// Ranges of an Org source that are markup rather than prose, for the
/// spellchecker to skip. Half-open [from, to) offsets into `text` (UTF-16).
/// Any order; overlaps allowed.
/// @param {string} text
/// @param {string} [key] names the document
/// @returns {Array<[number, number]>}
export function orgSkips(text, key) {
  /** @type {Array<[number, number]>} */
  const out = [];
  if (typeof text !== "string" || !text) return out;
  const skip = (from, to) => {
    if (to > from) out.push([from, to]);
  };
  orgTree(text, key).iterate({
    enter(ref) {
      const { name, from, to } = ref;
      if (WHOLE.has(name)) {
        skip(from, to);
        return false;
      }
      switch (name) {
        case "Keyword": {
          const value = ref.node.getChild("KeywordValue");
          const keyNode = ref.node.getChild("KeywordKey");
          const keyName = keyNode ? text.slice(keyNode.from, keyNode.to).split("[")[0].toUpperCase() : "";
          if (value && PROSE_KEYWORDS.has(keyName)) {
            skip(from, value.from);
            return true;
          }
          skip(from, to);
          return false;
        }
        case "Link": {
          const description = ref.node.getChild("LinkDescription");
          if (!description) {
            skip(from, to);
            return false;
          }
          skip(from, description.from);
          skip(description.to, to);
          return true;
        }
        case "FootnoteReference": {
          // [fn:label] is markup; in [fn:label:text] and [fn::text] only the
          // marker up to the text and the closing bracket are.
          const colon = text.slice(from, to).indexOf(":", 4);
          if (colon === -1) {
            skip(from, to);
            return false;
          }
          skip(from, from + colon + 1);
          if (text[to - 1] === "]") skip(to - 1, to);
          return true;
        }
        case "FootnoteDefinition": {
          // The label node holds the label alone; the brackets are markup too.
          const label = ref.node.getChild("FootnoteLabel");
          if (label) skip(label.from - 4, label.to + 1);
          return true;
        }
        case "Paragraph": {
          const begin = BEGIN_LINE.exec(text.slice(from, Math.min(to, from + 64)));
          if (!begin) return true;
          if (SKIP_BLOCKS.has(begin[1].toUpperCase())) {
            skip(from, to);
            return false;
          }
          const eol = text.indexOf("\n", from);
          skip(from, eol === -1 || eol > to ? to : eol);
          return true;
        }
        default:
          return true;
      }
    },
  });
  return out;
}
