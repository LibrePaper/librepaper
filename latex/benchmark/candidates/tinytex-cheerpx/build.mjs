// Repackage the existing TinyTeX userland; never modify the v86 candidate.
import {execFileSync} from 'node:child_process';
import {mkdirSync,readFileSync,writeFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';
import {CASES,treeOf} from '../../corpus.mjs';
const root=fileURLToPath(new URL('.',import.meta.url));
mkdirSync(root+'assets',{recursive:true});
if(!process.argv.includes('--receipt-only')) execFileSync('mke2fs',['-q','-t','ext2','-b','4096','-d',root+'../tinytex-v86/assets/rootfs',root+'assets/rootfs.ext2','1100M'],{stdio:'inherit'});
writeFileSync(root+'assets/receipt.json',JSON.stringify({
  date:new Date().toISOString(),runtime:'https://cxrtnc.leaningtech.com/1.2.8/cx.esm.js',
  sourceImage:JSON.parse(readFileSync(root+'../tinytex-v86/assets/image-receipt.json')),
  sha256:createHash('sha256').update(readFileSync(root+'assets/rootfs.ext2')).digest('hex')
},null,2)+'\n');
for(const id of ['multifile','biber-sorting']) {
  const example=CASES.find(c=>c.id===id),tree=treeOf(example);
  const source=root+'../tinytex-v86/results/'+id+'/native/';
  for(const [path,value] of Object.entries({...tree.texts,...tree.assets})) {
    if(!readFileSync(source+path).equals(Buffer.from(value))) throw new Error('Native input mismatch: '+id+'/'+path);
  }
  const target=root+'assets/reference/'+id+'/';
  mkdirSync(target,{recursive:true});
  for(const ext of ['pdf','bbl']) {
    const name=example.main.replace(/\.tex$/,'.'+ext);
    writeFileSync(target+name,readFileSync(source+name));
  }
}
