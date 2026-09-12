import assert from 'node:assert/strict';
import { mkdtemp,writeFile,readFile,rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { INSERT_ACTIONS,buildInsertion } from '../../src/lib/insert.js';
const run=promisify(execFile),dir=await mkdtemp(join(tmpdir(),'librepaper-insert-quarto-'));
try {
  await writeFile(join(dir,'plot.svg'),'<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>');
  await writeFile(join(dir,'refs.bib'),'@article{smith2020,author={Smith, Jane},title={Rivers},year={2020},journal={Journal}}');
  let text='---\ntitle: Insert check\nbibliography: refs.bib\n---\n\n# Intro {#sec-intro}\n\n';
  const options={title:'Example',src:'plot.svg',caption:'Results',label:'sample',rows:2,columns:2,header:true,alignment:'center',keys:['smith2020'],locator:'p. 3',file:'refs.bib',target:'sec-intro',brackets:'brackets',language:'python',url:'https://example.com',environment:'custom',numbered:true};
  for(const {id} of INSERT_ACTIONS){
    const c={format:'quarto',path:'main.qmd',text,mainText:text,selection:{from:text.length,to:text.length,text:''},files:[{path:'refs.bib',text:'bib'}]};
    const result=buildInsertion(id,options,c);
    assert.ok(result.text || result.additionalEdits.length,id+' must produce an edit');
    const edits=[...result.additionalEdits,{from:text.length,to:text.length,insert:result.text+'\n\n'}];
    for(const edit of edits.sort((a,b)=>b.from-a.from))text=text.slice(0,edit.from)+edit.insert+text.slice(edit.to);
  }
  const headerless=buildInsertion('table',{rows:2,columns:2,header:false,caption:'Headerless',label:'headerless'},{format:'quarto',path:'main.qmd',text,selection:{from:text.length,to:text.length,text:''}});
  text+=headerless.text+'\n';
  await writeFile(join(dir,'main.qmd'),text);
  const result=await run('quarto',['render','main.qmd','--to','html','--no-execute'],{cwd:dir,maxBuffer:2e6});
  assert.doesNotMatch(result.stderr,/ERROR|Unable to resolve|undefined cross-reference/i);
  const html=await readFile(join(dir,'main.html'),'utf8');
  for(const pattern of [/id="TOC"/,/class="citation"/,/class="csl-entry"/,/id="tbl-sample/,/id="thm-sample/,/class="math display"/,/<table/,/Headerless/,/plot.svg/])assert.match(html,pattern);
  console.log('insert-quarto-render: all 35 shared actions rendered; TOC, bibliography, citations, tables, theorems and math verified');
} catch(error) { console.error(error.stdout, error.stderr); throw error; } finally {await rm(dir,{recursive:true,force:true});}
