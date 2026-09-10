// Semantic actions shared by the navbar and the source-format adapters.
import { buildLatex } from './insert-latex.js';
import { buildTypst } from './insert-typst.js';
const item = (id, label, group, dialog, keywords = '') => ({ id, label, group, dialog, keywords });
export const INSERT_ACTIONS = [
  item('heading', 'Heading / section', 'Structure', 'heading', 'subsection subsubsection'),
  item('abstract', 'Abstract', 'Structure'), item('appendix', 'Appendix', 'Structure'), item('toc', 'Table of contents', 'Structure'),
  item('figure', 'Image / figure', 'Figures and tables', 'figure', 'includegraphics'), item('table', 'Table', 'Figures and tables', 'table', 'tabular'),
  item('citation', 'Citation', 'References', 'citation'), item('bibliography', 'Bibliography', 'References', 'bibliography'),
  item('cross-reference', 'Cross-reference', 'References', 'cross-reference'), item('label', 'Label / anchor', 'References', 'label'),
  item('inline-math', 'Inline math', 'Math'), item('display-math', 'Displayed equation', 'Math', 'math', 'equation'),
  item('aligned-math', 'Aligned equations', 'Math', 'math', 'align aligned'), item('gather-math', 'Gathered equations', 'Math', 'math', 'gather gathered'),
  item('cases', 'Cases', 'Math', 'matrix'), item('matrix', 'Matrix', 'Math', 'matrix', 'pmatrix bmatrix Bmatrix vmatrix Vmatrix'),
  item('bulleted-list', 'Bulleted list', 'Lists', null, 'itemize'), item('numbered-list', 'Numbered list', 'Lists', null, 'enumerate'), item('description-list', 'Description list', 'Lists', null, 'description terms'),
  item('quote', 'Block quotation', 'Text blocks', null, 'quote'), item('quotation', 'Indented quotation', 'Text blocks', null, 'quotation'),
  item('code-block', 'Code block', 'Text blocks', 'code', 'verbatim listings'), item('footnote', 'Footnote', 'Text blocks'), item('link', 'Link', 'Text blocks', 'link'),
  ...['theorem', 'lemma', 'proposition', 'definition', 'proof', 'example', 'remark'].map(id => item(id, id[0].toUpperCase() + id.slice(1), 'Scholarly blocks', 'environment')),
  item('page-break', 'Page break', 'Layout'), item('horizontal-rule', 'Horizontal rule', 'Layout'), item('columns', 'Columns', 'Layout', 'columns', 'multicols'),
  item('custom-environment', 'Environment', 'Advanced', 'environment', 'custom newenvironment newtheorem'),
];
const formats = new Set(['latex', 'typst', 'markdown', 'quarto']);
const scholarly = new Set(['theorem','lemma','proposition','definition','proof','example','remark']);
function relativePath(path, base) {
  if (/^(?:[a-z]+:|\/)/i.test(path)) return path;
  const from=(base || '').split('/').slice(0,-1),to=path.split('/');
  while(from.length && to.length && from[0]===to[0]){from.shift();to.shift();}
  return [...from.map(()=>'..'),...to].join('/');
}
const inline = new Set(['inline-math','citation','cross-reference','label','footnote','link']);
const plain = value => String(value ?? '');
const markdown = value => plain(value).replace(/[\\`*_{}\[\]<>]/g, '\\$&');
const html = value => plain(value).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const slug = value => plain(value).trim().replace(/[^\p{L}\p{N}_.:-]+/gu, '-').replace(/^-|-$/g, '') || 'item';
const url = value => { const s=plain(value).trim(); if (/^(?:javascript|data|vbscript):/i.test(s)) throw Error('Use a project path or an http, https, or mailto link.'); return s.replace(/[\s<>"()]/g,c=>encodeURIComponent(c)); };
function context(c = {}) { const text=plain(c.text); const selection=c.selection || {from:text.length,to:text.length,text:''}; return {...c, text, selection, files:c.files || []}; }
export function insertSyntaxContext(input) {
  const c=context(input), before=c.text.slice(0,c.selection.from), format=c.format;
  if (format === 'markdown' || format === 'quarto' || format === 'typst') {
    let fence=null;
    for (const line of before.split('\n')) {
      const m=line.match(/^\s*(`{3,}|~{3,})(.*)$/); if(!m)continue;
      if(!fence)fence=m[1]; else if(m[1][0]===fence[0] && m[1].length>=fence.length && !m[2].trim()) fence=null;
    }
    if(fence)return 'code';
    if((format==='markdown'||format==='quarto') && /^---\r?\n/.test(c.text)) {
      const closing=c.text.slice(4).search(/^(?:---|\.\.\.)\s*$/m); if(closing>=0 && before.length<closing+7)return 'metadata';
    }
    if(format!=='typst' && before.lastIndexOf('<!--')>before.lastIndexOf('-->'))return 'comment';
    if(format==='typst' && (before.lastIndexOf('/*')>before.lastIndexOf('*/') || /(^|[^:])\/\/[^\n]*$/.test(before)))return 'comment';
    const line=before.slice(before.lastIndexOf('\n')+1);
    if((line.match(/(?<!\\)`/g)||[]).length%2)return 'code';
    if(format==='typst') {
      // Code expressions contain markup only inside content blocks. Refuse
      // raw expressions instead of inserting markup into function arguments.
      const start=before.lastIndexOf('#');
      if(start>=0){const tail=before.slice(start); if(/^#[\w.-]*\([^\[\]]*$/.test(tail) && (tail.match(/\(/g)||[]).length>(tail.match(/\)/g)||[]).length)return 'code';}
    }
  }
  if(format==='latex') {
    if(/(^|[^\\])%[^\n]*$/.test(before))return 'comment';
    for(const env of ['verbatim','Verbatim','lstlisting','minted'])if(before.lastIndexOf(`\\begin{${env}}`)>before.lastIndexOf(`\\end{${env}}`))return 'code';
    if(before.lastIndexOf('\\(')>before.lastIndexOf('\\)') || before.lastIndexOf('\\[')>before.lastIndexOf('\\]'))return 'math';
    for(const env of ['equation','equation*','align','align*','gather','gather*'])if(before.lastIndexOf(`\\begin{${env}}`)>before.lastIndexOf(`\\end{${env}}`))return 'math';
  }
  const dollars=before.replace(/\\\$/g,'').match(/\$\$|\$/g)||[];
  let delimiter=null; for(const d of dollars){if(!delimiter)delimiter=d;else if(d===delimiter)delimiter=null;}
  return delimiter?'math':'markup';
}
export function insertionAvailability(id, input = {}) {
  const c=context(input);
  if(!INSERT_ACTIONS.some(a=>a.id===id))return {enabled:false,reason:'Unknown insertion.'};
  if(!formats.has(c.format))return {enabled:false,reason:'Open a LaTeX, Typst, Markdown, or Quarto source file.'};
  const mode=insertSyntaxContext(c);
  if(['code','comment','metadata'].includes(mode))return {enabled:false,reason:`Move the caret out of the ${mode==='metadata'?'document metadata':mode+' block'} to insert document content.`};
  if(mode==='math' && !['matrix','cases','label'].includes(id))return {enabled:false,reason:'Move outside the current equation to insert this item.'};
  if(id==='custom-environment' && !['latex','typst','quarto'].includes(c.format))return {enabled:false,reason:'Custom environments need LaTeX, Typst, or Quarto.'};
  if(id==='custom-environment' && c.format==='typst' && !gatherInsertEnvironments(c).length)return {enabled:false,reason:'Define a content function in this Typst project first.'};
  if(id==='cross-reference' && !gatherInsertTargets(c).length)return {enabled:false,reason:'Add a heading or a labelled element first.'};
  return {enabled:true};
}
export function gatherInsertEnvironments(input = {}) {
  const c=context(input), text=[c.text,c.mainText,...c.files.map(f=>f.text)].filter(Boolean).join('\n'), names=new Set();
  const regex=c.format==='latex'?/\\(?:newenvironment|renewenvironment|newtheorem)\*?\s*\{([^}]+)\}/g:/#let\s+([A-Za-z][\w-]*)\s*\([^)]*\bbody\b[^)]*\)/g;
  for(const m of text.matchAll(regex))names.add(m[1]);
  return [...names].sort();
}
export function gatherInsertTargets(input = {}) {
  const c=context(input), found=[], seen=new Set();
  const sources=[{path:c.path,text:c.text},...c.files.filter(f=>f.path!==c.path && typeof f.text==='string')];
  function add(id,title,kind,path){if(!seen.has(id)){seen.add(id);found.push({id,label:title||id,kind,path});}}
  for(const source of sources){
    const text=plain(source.text).replace(/^\s*(`{3,}|~{3,})[^\n]*\n[\s\S]*?^\s*\1\s*$/gm,'');
    if(c.format==='latex'){
      for(const m of text.replace(/(?<!\\)%[^\n]*/g,'').matchAll(/\\label\s*\{([^}]+)\}/g))add(m[1],m[1],'label',source.path);
    }else if(c.format==='typst'){
      for(const m of text.matchAll(/<([\p{L}\p{N}_.:-]+)>/gu))add(m[1],m[1],'label',source.path);
    }else{
      const counts=new Map();
      for(const m of text.matchAll(/^#{1,6}\s+(.+?)(?:\s+\{([^}]+)\})?\s*$/gm)){
        const explicit=m[2]?.match(/#([^\s}]+)/)?.[1];
        const title=m[1].replace(/[*_`]/g,'');
        const base=title.toLowerCase().replace(/[^\p{L}\p{N}_\-\s]/gu,'').replace(/\s/g,'-');
        const n=counts.get(base)||0;counts.set(base,n+1);
        add(explicit||base+(n?`-${n}`:''),title,'heading',source.path);
      }
      for(const m of text.matchAll(/\{[^}\n]*#([\w:.-]+)[^}]*\}|<a\s+(?:id|name)=["']([^"']+)["']/g))add(m[1]||m[2],m[1]||m[2],'label',source.path);
    }
  }
  return found;
}
function uniqueLabel(value,c,prefix='') {
  let base=slug(value), used=new Set(gatherInsertTargets(c).map(x=>x.id));
  if(prefix && !base.startsWith(prefix+'-'))base=prefix+'-'+base;
  let result=base,n=2;while(used.has(result))result=base+'-'+n++;
  return result;
}
// Metadata edits address the captured source, never a generated snippet.
function metadata(c, additions) {
  const source=c.text, match=source.match(/^---\r?\n([\s\S]*?)\r?\n(?:---|\.\.\.)(?:\r?\n|$)/);
  const pending=Object.entries(additions).filter(([key])=>!new RegExp('^'+key+'\\s*:','m').test(match?.[1]||''));
  if(!pending.length)return [];
  const insert=pending.map(([key,value])=>`${key}: ${value}`).join('\n')+'\n';
  return [{from:match?source.indexOf('\n')+1:0,to:match?source.indexOf('\n')+1:0,insert:match?insert:`---\n${insert}---\n\n`}];
}
function markdownMath(id,o,c) {
  const body=c.selection.text||'x = y', rowCount=o.rows||2, columns=o.columns||2;
  let formula=body;
  if(id==='aligned-math')formula=`\\begin{aligned}\n${c.selection.text||'x &= y \\\\\nz &= w'}\n\\end{aligned}`;
  if(id==='gather-math')formula=`\\begin{gathered}\n${c.selection.text||'x = y \\\\\nz = w'}\n\\end{gathered}`;
  if(id==='cases')formula=`\\begin{cases}\n${Array.from({length:rowCount},(_,i)=>`${i+1} & x ${i?' >':' \\le'} 0`).join(' \\\\\n')}\n\\end{cases}`;
  if(id==='matrix'){
    const env=({parentheses:'pmatrix',brackets:'bmatrix',braces:'Bmatrix',bars:'vmatrix',doublebars:'Vmatrix',none:'matrix'})[o.brackets]||'pmatrix';
    formula=`\\begin{${env}}\n${Array.from({length:rowCount},()=>Array(columns).fill('0').join(' & ')).join(' \\\\\n')}\n\\end{${env}}`;
  }
  const text=insertSyntaxContext(c)==='math'?formula:id==='inline-math'?`$${formula}$`:`$$\n${formula}\n$$${c.format==='quarto' && o.numbered!==false && o.label?` {#${o.label}}`:''}`;
  return {text,placeholder:id==='matrix'?'0':id==='cases'?'1':body};
}
function buildMarkdown(id,o,c) {
  const q=c.format==='quarto', selected=c.selection.text||'', body=selected||'Content.', notes=[], additionalEdits=[];
  let text='',placeholder=body;
  if(id==='heading'){
    placeholder=markdown(o.title||selected||'Heading');text='#'.repeat(o.level||1)+' '+placeholder;
    if(q){text+=` {${o.label?'#'+o.label+' ':''}${o.numbered===false?'.unnumbered':''}}`.replace(' {}','');if(o.numbered===true)additionalEdits.push(...metadata(c,{'number-sections':'true'}));}
    else if(o.label)text=`<a id="${html(o.label)}"></a>\n\n`+text;
  }else if(id==='abstract')text=q?`::: {.abstract}\n\n${selected||'Abstract text.'}\n\n:::`:`## Abstract\n\n${selected||'Abstract text.'}`;
  else if(id==='appendix')text=q?'## Appendix {.appendix}':'## Appendix';
  else if(id==='toc'){
    if(q){additionalEdits.push(...metadata(c,{toc:'true'}));text='';notes.push('Enable the table of contents in document metadata.');}
    else {text=gatherInsertTargets(c).filter(t=>t.kind==='heading').map(t=>`- [${markdown(t.label)}](#${url(t.id)})`).join('\n');if(!text)throw Error('Add a heading before inserting a table of contents.');notes.push('This table of contents contains links to the current headings.');}
  }else if(id==='figure'){
    if(!o.src)throw Error('Choose an image first.');
    const caption=markdown(o.caption||''); text=`![${caption}](${url(o.src)})`;
    if(q)text+=`{${[o.label?'#'+o.label:'',o.width?`width="${o.width}"`:''].filter(Boolean).join(' ')}}`.replace('{}','');
    else if(o.width || o.label || o.caption)text=`<figure${o.label?` id="${html(o.label)}"`:''}>\n<img src="${html(url(o.src))}" alt="${html(o.caption)}"${o.width?` style="width: ${o.width}"`:''}>${o.caption?`\n<figcaption>${html(o.caption)}</figcaption>`:''}\n</figure>`;
  }else if(id==='table'){
    const r=o.rows||3,m=o.columns||3, header=o.header!==false;
    const cells=Array.from({length:r},(_,i)=>Array.from({length:m},(_,j)=>i===0&&header?`Header ${j+1}`:`Cell ${i+1},${j+1}`));
    if(!header){text=`<table${o.label?` id="${html(o.label)}"`:''}>${o.caption?`\n<caption>${html(o.caption)}</caption>`:''}\n<tbody>\n${cells.map(row=>'<tr>'+row.map(cell=>`<td${o.alignment!=='default'?` style="text-align: ${o.alignment}"`:''}>${cell}</td>`).join('')+'</tr>').join('\n')}\n</tbody>\n</table>`; if(q)notes.push('A table without a header uses HTML and is intended for HTML output.');}
    else {
      const align=({left:':---',center:':---:',right:'---:'})[o.alignment]||'---';
      text=[cells[0],Array(m).fill(align),...cells.slice(1)].map(row=>'| '+row.join(' | ')+' |').join('\n');
      if(q&&(o.caption||o.label))text+=`\n\n: ${markdown(o.caption||'Table')}${o.label?` {#${o.label}}`:''}`;
      else if(o.label||o.caption)text=(o.label?`<a id="${html(o.label)}"></a>\n\n`:'')+text+(o.caption?`\n\n${markdown(o.caption)}`:'');
    }
    placeholder=cells[0][0];
  }else if(id==='citation'){
    if(!o.keys?.length)throw Error('Select at least one reference.');
    const keys=o.keys.map(key=>{if(!/^[\p{L}\p{N}_:.+-]+$/u.test(key))throw Error('This citation key contains unsupported characters.');return '@'+key;});
    text=o.style==='narrative'?keys.join('; ')+(o.locator?` [${markdown(o.locator)}]`:''):`[${keys.join('; ')}${o.locator?', '+markdown(o.locator):''}]`;
    if(!/^bibliography\s*:/m.test(c.text)){
      const files=c.files.filter(f=>/\.(bib|json|ya?ml)$/i.test(f.path));
      if(files.length===1)additionalEdits.push(...metadata(c,{bibliography:JSON.stringify(relativePath(files[0].path,c.path))}));
      else throw Error('Insert a bibliography first to choose the reference source.');
    }
  }else if(id==='bibliography'){
    if(!o.file)throw Error('Choose a bibliography file.');
    const existing=c.text.match(/^bibliography\s*:\s*(.+)$/m);
    if(existing && !existing[1].includes(o.file))throw Error('This document already names another bibliography. Update its metadata to combine or replace sources.');
    additionalEdits.push(...metadata(c,{bibliography:JSON.stringify(o.file)}));
    text=q?'::: {#refs}\n:::':'## References';
  }else if(id==='cross-reference'){
    if(!o.target)throw Error('Choose a reference target.');
    const target=gatherInsertTargets(c).find(t=>t.id===o.target);
    text=q && /^(sec|fig|tbl|eq|thm|lem|prp|def|exm)-/.test(o.target)?'@'+o.target:`[${markdown(target?.label||o.target)}](#${url(o.target)})`;
  }else if(id==='label')text=q?`[]{#${o.label}}`:`<a id="${html(o.label)}"></a>`;
  else if(['inline-math','display-math','aligned-math','gather-math','cases','matrix'].includes(id))return markdownMath(id,o,c);
  else if(['bulleted-list','numbered-list','description-list'].includes(id)){
    const lines=(selected||'First item\nSecond item').split(/\r?\n/).filter(s=>s.trim());
    if(id==='description-list')text=q?lines.map(line=>`${line}\n:   Description.`).join('\n\n'):`<dl>\n${lines.map(line=>`<dt>${html(line)}</dt>\n<dd>Description.</dd>`).join('\n')}\n</dl>`;
    else text=lines.map((line,i)=>(id==='bulleted-list'?'- ':`${i+1}. `)+line).join('\n');
    placeholder=id==='description-list'?'Description.':lines[0];
  }else if(id==='quote'||id==='quotation')text=(selected||'Quotation.').split('\n').map(line=>'> '+line).join('\n');
  else if(id==='code-block'){
    const content=selected||'code', longest=Math.max(2,...[...content.matchAll(/`+/g)].map(m=>m[0].length));const fence='`'.repeat(longest+1);
    text=fence+(o.language||'')+'\n'+content+'\n'+fence;placeholder=content;
  }else if(id==='footnote'){
    if(q)text=`^[${selected||'Note.'}]`;
    else {let n=1;while(c.text.includes(`[^note-${n}]`))n++;text=`[^note-${n}]`;additionalEdits.push({from:c.text.length,to:c.text.length,insert:`\n\n[^note-${n}]: ${(selected||'Note.').replace(/\n/g,'\n    ')}\n`});}
  }else if(id==='link'){placeholder=selected||markdown(o.title||'Link');text=`[${placeholder}](${url(o.url||'https://example.com')})`;}
  else if(scholarly.has(id)){
    const title=id[0].toUpperCase()+id.slice(1), heading=o.title?markdown(o.title):title;
    if(q){const attr=['proof','remark'].includes(id)?`.${id}${o.label?' #'+o.label:''}`:'#'+o.label;text=`::: {${attr}}\n\n${o.title?'## '+heading+'\n\n':''}${body}\n\n:::`;}
    else text=`${o.label?`<a id="${html(o.label)}"></a>\n\n`:''}**${title}${o.title?` (${heading})`:''}.** ${body}`;
  }else if(id==='page-break')text=q?'{{< pagebreak >}}':'<div style="break-after: page;"></div>';
  else if(id==='horizontal-rule')text='---';
  else if(id==='columns'){
    const count=o.columns||2, gap=o.gap||'1em';
    text=q?`:::: {layout-ncol=${count}}\n\n${Array.from({length:count},(_,i)=>`::: {}\n${i===0?body:'Column content.'}\n:::`).join('\n\n')}\n\n::::`:`<div style="column-count: ${count}; column-gap: ${gap}">\n<p>${html(body)}</p>\n</div>`;
    if(q&&o.gap)notes.push('Quarto panel spacing follows the document theme.');
  }else if(id==='custom-environment'){
    if(!/^[A-Za-z][\w-]*$/.test(o.environment||''))throw Error('Enter an environment name using letters, numbers, or hyphens.');
    text=`::: {.${o.environment}}\n\n${body}\n\n:::`;notes.push('This div uses the styling defined by your document or extension.');
  }
  return {text,placeholder,additionalEdits,notes};
}
export function buildInsertion(id, options = {}, input = {}) {
  const c=context(input), available=insertionAvailability(id,c);if(!available.enabled)throw Error(available.reason);
  const o={...options, inMath: insertSyntaxContext(c)==='math'};
  for(const [key,max] of [['rows',100],['columns',30],['level',6]])if(o[key]!=null){const n=Number(o[key]);if(!Number.isInteger(n)||n<1||n>max)throw Error(`${key[0].toUpperCase()+key.slice(1)} must be between 1 and ${max}.`);o[key]=n;}
  if(o.width && !/^\d+(?:\.\d+)?(?:%|cm|mm|in|pt|px|em)$/.test(o.width) && !(c.format==='latex' && /^(?:\d*\.?\d+)?\\(?:line|text)width$/.test(o.width)))throw Error('Use a width such as 80%, 8cm, or 200pt.');
  if(o.gap && !/^\d+(?:\.\d+)?(?:cm|mm|in|pt|px|em)$/.test(o.gap))throw Error('Use spacing such as 1em or 12pt.');
  if(o.language && !/^[\w.+-]+$/.test(o.language))throw Error('Use a language name such as python, r, or javascript.');
  if(o.alignment && !['default','left','center','right'].includes(o.alignment))throw Error('Choose left, center, or right alignment.');
  const prefixes={heading:'sec',figure:'fig',table:'tbl','display-math':'eq','aligned-math':'eq','gather-math':'eq',theorem:'thm',lemma:'lem',proposition:'prp',definition:'def',example:'exm',proof:'prf',remark:'rem',cases:'eq',matrix:'eq'};
  if(o.label || id==='label' || (c.format==='quarto'&&scholarly.has(id)&&!['proof','remark'].includes(id)))o.label=uniqueLabel(o.label||id,c,c.format==='quarto'?prefixes[id]:'');
  const basePath=c.format==='latex' ? c.mainPath || c.path : c.path;
  if(o.src)o.src=relativePath(o.src,basePath);
  if(o.file)o.file=relativePath(o.file,basePath);
  if(o.url)url(o.url);
  let result=c.format==='latex'?buildLatex(id,o,c):c.format==='typst'?buildTypst(id,o,c):buildMarkdown(id,o,c);
  if(typeof result==='string')result={text:result};
  let text=result.text||'',prefix='',suffix='';
  if(text && !inline.has(id) && insertSyntaxContext(c)!=='math'){
    const before=c.text.slice(0,c.selection.from),after=c.text.slice(c.selection.to);
    prefix=before && !before.endsWith('\n\n')?(before.endsWith('\n')?'\n':'\n\n'):'';
    suffix=after && !after.startsWith('\n\n')?(after.startsWith('\n')?'\n':'\n\n'):'';
  }
  const needle=result.placeholder || c.selection.text;
  const at=needle?text.indexOf(needle):-1;
  const selection=at>=0?{anchor:prefix.length+at,head:prefix.length+at+needle.length}:{anchor:prefix.length+text.length,head:prefix.length+text.length};
  const additionalEdits=[];
  for(const edit of result.additionalEdits||[]){
    const previous=additionalEdits.find(e=>e.path===edit.path&&e.from===edit.from&&e.to===edit.to&&e.from===e.to);
    if(previous)previous.insert+=edit.insert;else additionalEdits.push({...edit});
  }
  return {...result,text:prefix+text+suffix,selection,notes:result.notes||[],additionalEdits};
}
