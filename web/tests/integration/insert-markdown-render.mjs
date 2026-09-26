import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import katex from 'katex';
import { call, handOver } from '../../src/lib/renderer-wasm.js';
import { INSERT_ACTIONS,buildInsertion,insertionAvailability } from '../../src/lib/insert.js';
const binary=readFileSync(new URL('../../dist/wasm/citations.wasm',import.meta.url));
const engine=new WebAssembly.Instance(new WebAssembly.Module(binary),{}).exports;
const bib='@article{smith2020,author={Smith, Jane},title={Rivers},year={2020},journal={Journal}}';
let text='---\nbibliography: refs.bib\nbibliography-style: apa\n---\n\n# Intro\n\n';
const options={title:'Example',src:'plot.png',caption:'Results',label:'sample',rows:2,columns:2,header:true,alignment:'center',keys:['smith2020'],locator:'p. 3',file:'refs.bib',target:'intro',brackets:'brackets',language:'python',url:'https://example.com',numbered:true};
let count=0;
for(const {id} of INSERT_ACTIONS) {
  const c={format:'markdown',path:'paper.md',text,selection:{from:text.length,to:text.length,text:''},files:[{path:'refs.bib',text:bib}]};
  if(!insertionAvailability(id,c).enabled)continue;
  const result=buildInsertion(id,options,c);
  assert.ok(result.text || result.additionalEdits.length,id);
  if(['inline-math','display-math','aligned-math','gather-math','matrix','cases'].includes(id))katex.renderToString(result.text.trim().replace(/<[^>]*>/g,'').replaceAll('&amp;','&').replaceAll('&lt;','<').replaceAll('&gt;','>').replaceAll('&quot;','"').replaceAll('&#39;',"'"),{throwOnError:true});
  const edits=[...result.additionalEdits,{from:text.length,to:text.length,insert:result.text+'\n\n'}];
  for(const edit of edits.sort((a,b)=>b.from-a.from))text=text.slice(0,edit.from)+edit.insert+text.slice(edit.to);
  count++;
}
handOver(engine,{main:'paper.md',texts:{'paper.md':text,'refs.bib':bib},assets:{},urls:{}});
const rendered=call(engine,'compile',text,'Insert check');
assert.equal(rendered.ok,true);assert.deepEqual(rendered.diagnostics,[]);
for(const pattern of [/<table/,/class="citation"/,/Rivers/,/footnote/,/data-math-style="display"/,/<dl>/,/href="#intro"/,/<blockquote>/])assert.match(rendered.text,pattern);
assert.equal(count,34);
console.log('insert-markdown-render: all 34 available actions rendered through LibrePaper WASM; math validated with KaTeX');
