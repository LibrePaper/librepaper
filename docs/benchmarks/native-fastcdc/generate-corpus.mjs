import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
const root = process.argv[2];
const repo = process.argv[3] ?? process.cwd();
if (!root) throw new Error('Usage: node generate-corpus.mjs OUTPUT_DIRECTORY [REPOSITORY]');
fs.mkdirSync(root, {recursive:true});
const sourceCommit = '75701a2b6744e788d0b7f0409764acf0169ddc13';
const readTracked = file => execFileSync('git',['show',`${sourceCommit}:${file}`],{cwd:repo,encoding:'utf8'});
const sha = b => crypto.createHash('sha256').update(b).digest('hex');
const cases = [];
function save(name, snapshots, description) {
  const dir = path.join(root, 'corpus', name); fs.mkdirSync(dir, {recursive:true});
  const versions = snapshots.map((s,i) => {
    const bytes = Buffer.from(s); const file = path.join(dir, `${String(i).padStart(4,'0')}.txt`);
    fs.writeFileSync(file, bytes); return {file, sha:sha(bytes), size:bytes.length};
  });
  cases.push({name, description, versions});
}
function edits(initial, count, mode) {
  let text = initial; const out = [text];
  for (let i=1;i<=count;i++) {
    if (mode === 'front-insert') text = `Revision note ${i}: a newly added observation.\n` + text;
    else {
      const words = [...text.matchAll(/[A-Za-z]{4,}/g)];
      const n = mode === 'scattered' ? 12 : 1;
      const picks = new Map();
      for(let j=0;j<n;j++) {
        const k = mode === 'localized' ? Math.floor(words.length * .51) : ((i*7919+j*1543) % words.length);
        const w=words[k]; picks.set(w.index, {length:w[0].length, replacement:`revision${i}word${j}`});
      }
      for(const [at,p] of [...picks].sort((a,b)=>b[0]-a[0])) text=text.slice(0,at)+p.replacement+text.slice(at+p.length);
    }
    out.push(text);
  }
  return out;
}
for(const [name,file] of [['small-markdown','examples/tutorial-markdown/librepaper.md'],['small-typst','examples/tutorial-typst/librepaper.typ'],['small-latex','examples/tutorial-latex/librepaper.tex']]) {
  save(name,edits(readTracked(file),200,'localized'),`200 synthetic one-word edits to real ${file}; compression corpus, not a compilation test`);
}
const readme=readTracked('README.md');
for(const mode of ['localized','scattered','front-insert']) save(`large-${mode}`,edits(readme,200,mode),`200 synthetic ${mode} edits to the real 61 KB README; not artificial repetitions of a small file`);
const commits=execFileSync('git',['log','-50','--format=%H',sourceCommit,'--','README.md'],{cwd:repo,encoding:'utf8'}).trim().split('\n').filter(Boolean).reverse();
const history=commits.map(c=>execFileSync('git',['show',`${c}:README.md`],{cwd:repo}));
if(history.length>1) save('real-readme-history',history,'Up to 50 actual README revisions from Git, oldest first; source histories only, not a whole-project benchmark');
fs.writeFileSync(path.join(root,'corpus.json'),JSON.stringify({repo,head:sourceCommit,cases},null,2));
console.log(JSON.stringify(cases.map(c=>({name:c.name,versions:c.versions.length,firstBytes:c.versions[0].size,logicalBytes:c.versions.reduce((n,v)=>n+v.size,0)})),null,2));
