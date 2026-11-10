import { TableAdapter } from './language';
import { cellAt, formatPipeTable, parsePipeTable, pipeBlock, tableLines } from '../../pipe-tables.js';

type Row = { kind: 'cells'; cells: string[] } | { kind: 'rule'; align?: Array<'left' | 'center' | 'right' | null> };

// The table adapter for a pipe table. The cells rows of the table block inside the token are the
// rows the editor shows. Rule rows keep their place, and any text around the block (a Markdown
// caption, an Org #+TBLFM line) is left as written.
export function pipeTableAdapter(flavor: 'org' | 'markdown'): TableAdapter {
  return {
    parse(source, token) {
      const block = pipeBlock(source.slice(token.from, token.to));
      if (!block) return [];
      return parsePipeTable(block.text).rows.flatMap(row => (row.kind === 'cells' ? [row.cells] : []));
    },
    locate(source, token, pos) {
      const block = pipeBlock(source.slice(token.from, token.to));
      if (!block) return null;
      const rel = pos - token.from - block.from;
      if (rel < 0 || rel > block.text.length) return null;
      const at = cellAt(block.text, rel);
      if (!at) return null;
      // The row index counts cells rows only, as parse does.
      const row = tableLines(block.text).slice(0, at.row).filter(line => !line.rule).length;
      return { row, col: at.col };
    },
    serialize(cells, token, text) {
      const block = pipeBlock(text);
      if (!block || !cells.length) return text;
      const { rows: original, indent } = parsePipeTable(block.text);

      let rows: Row[];
      if (flavor === 'markdown') {
        // Markdown has one separator, under the header, with the alignment the table had.
        const align = original.find(row => row.kind === 'rule' && row.align)?.align;
        rows = [
          { kind: 'cells', cells: cells[0] },
          { kind: 'rule', align },
          ...cells.slice(1).map(row => ({ kind: 'cells' as const, cells: row }))
        ];
      } else {
        // Org keeps each rule row where it was; rows added past the original end follow the last one.
        rows = [];
        let next = 0;
        for (const row of original) {
          if (row.kind === 'rule') rows.push(row);
          else if (next < cells.length) rows.push({ kind: 'cells', cells: cells[next++] });
        }
        for (; next < cells.length; next++) rows.push({ kind: 'cells', cells: cells[next] });
      }

      const formatted = formatPipeTable(rows, flavor, indent);
      return text.slice(0, block.from) + formatted + text.slice(block.to);
    }
  };
}
