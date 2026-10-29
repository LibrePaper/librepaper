import { Rule } from '../../core/language';
import { div, fence, footnoteDefinition, frontmatter, heading, hr, indentedCode, linkDefinition, list, quote, setext, table } from './block';
import { autolink, bareUrl, bracket, cite, code, comment, emphasis, escape, image, linebreak, math, shortcode } from './inline';

// Block rules first: they only match at a line start (or after quote markers), so the
// order decides between constructs that begin with the same character. Inline rules
// follow and run at every other position inside the text.
export const rules: Rule[] = [
  frontmatter, indentedCode, fence, div, table, quote, heading, hr, list,
  footnoteDefinition, linkDefinition, setext,
  linebreak, escape, code, math, comment, shortcode, image, bracket, cite, autolink, bareUrl, emphasis
];
