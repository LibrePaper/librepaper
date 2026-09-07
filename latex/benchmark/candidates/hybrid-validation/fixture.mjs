import { treeOf } from "../../corpus.mjs";
export const EXAMPLE={id:"biber-sorting",source:"biblatex",main:"91-sorting-schemes.tex",engine:"xelatex"};
export const PHASES=["first","prose-edit","citation-addition","title-edit"];
export const MARKERS={first:"HYBRID BASELINE Ω café","prose-edit":"HYBRID PROSE EDIT Ω café","citation-addition":"HYBRID CITATION ADDITION Ω café","title-edit":"HYBRID title edit"};
const append=(s,x)=>{const i=s.lastIndexOf("\\end{document}");if(i<0)throw Error("No end document");return s.slice(0,i)+x+"\n"+s.slice(i)};
const beforeBibliography=(s,x)=>{const i=s.indexOf("\\newrefcontext");if(i<0)throw Error("No bibliography context");return s.slice(0,i)+x+"\n"+s.slice(i)};
export function fixture(phase){
  if(!PHASES.includes(phase))throw Error(`Unknown phase: ${phase}`);
  const t=treeOf(EXAMPLE), mainBase=t.texts[t.main]; let m=append(mainBase,`\\par ${MARKERS.first}`);
  if(phase!=="first")m=append(m,`\\par ${MARKERS["prose-edit"]}: A prose edit with café and Ω.`);
  if(phase==="citation-addition"||phase==="title-edit")m=beforeBibliography(m,`\\par ${MARKERS["citation-addition"]}: \\parencite{glashow}`);
  if(phase==="title-edit"){
    // Keep the TeX tree identical to citation-addition; this isolates the bibliography edit.
    const b=t.texts["biblatex-examples.bib"],e=b.indexOf("@book{companion,"),s=b.indexOf("  title        = ",e),n=b.indexOf("\n",s);
    if(e<0||s<0||n<0)throw Error("Cannot locate companion title");t.texts["biblatex-examples.bib"]=b.slice(0,s)+"  title        = {HYBRID title edit: The LaTeX Companion},"+b.slice(n);
  }
  t.texts[t.main]=m;return t;
}
export const inputFiles=t=>({...t.texts,...t.assets});
