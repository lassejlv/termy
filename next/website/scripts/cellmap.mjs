// The libtermy section's cell map: node scripts/cellmap.mjs public/images/cellmap.svg
import { writeFileSync } from 'node:fs';
const L = [
 ['> give the pane header its own drag handle',''],
 ['',''],
 ['● Read Sources/TermyNext/PaneView.swift',''],
 ['● Read Sources/TermyNext/PaneDrag.swift',''],
 ['● Update PaneView.swift',''],
 ['  ⎿  +42 −17',''],
 ['● Bash just test-next',''],
 ['  ⎿  38 tests passed',''],
 ['● Update SplitContainerView.swift','all'],
 ['  ⎿  +8 −3','all'],
 ['',''],
 ['✻ Splitting PaneView… 1m 13s, esc to interrupt','0,26'],
];
const cols=48, rows=L.length, cw=12, ch=26, W=cols*cw, H=rows*ch;
let s=`<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">`;
L.forEach(([t,m],r)=>{const chars=[...t];const lit=m==='all'?null:new Set(m?m.split(',').map(Number):[]);
 for(let c=0;c<cols;c++){const ch_=chars[c]??' ';const isLit=c<chars.length&&(m==='all'?ch_!==' '||c<chars.length:lit.has(c));
  const x=c*cw+cw/2,y=r*ch+ch/2;
  if(isLit) s+=`<rect x="${x-5}" y="${y-10}" width="10" height="20" rx="2" fill="#7AA2F7"/>`;
  else if(ch_!==' ') s+=`<rect x="${x-2.5}" y="${y-2.5}" width="5" height="5" rx="1" fill="#414868"/>`;
  else s+=`<circle cx="${x}" cy="${y}" r="1.2" fill="#1F2335"/>`;}});
s+='</svg>';writeFileSync(process.argv[2] ?? "cellmap.svg", s);console.log(W,H);
