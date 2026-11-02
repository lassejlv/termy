// Halftone sun as terminal cells: node scripts/halftone-sun.mjs <w> <h> <cx> <cy> <r> <out.svg>, then rasterize (rsvg-convert -z 2) and convert to webp.
// Halftone sunset: terminal-cell grid, dot size from distance to the sun.
import { writeFileSync } from 'node:fs';
const W = +process.argv[2], H = +process.argv[3], cx = +process.argv[4], cy = +process.argv[5], R = +process.argv[6], out = process.argv[7];
const cellW = 9, cellH = 12; // terminal cell aspect
const stops = [[0,[180,204,255]],[0.45,[122,162,247]],[1,[61,89,161]]]; // Termy: #b4ccff, #7aa2f7, #3d59a1
const mix=(t)=>{for(let i=1;i<stops.length;i++){if(t<=stops[i][0]){const [a,ca]=stops[i-1],[b,cb]=stops[i];const k=(t-a)/(b-a);return ca.map((v,j)=>Math.round(v+(cb[j]-v)*k));}}return stops.at(-1)[1];};
let s=`<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">`;
for(let y=cellH/2;y<H;y+=cellH)for(let x=cellW/2;x<W;x+=cellW){
  const d=Math.hypot(x-cx,(y-cy)*1.0)/R;
  let r;
  if(d<=1) r=3.7-1.1*d*d; else r=1.9*Math.pow(Math.max(0,1-(d-1)/0.75),2.2);
  if(r<0.45) continue;
  const t=Math.min(1,Math.max(0,(y-(cy-R))/(2*R)));
  const [cr,cg,cb]=mix(t);
  const o=d<=1?0.95:Math.max(0.2,0.7-(d-1)*0.9);
  s+=`<circle cx="${x}" cy="${y}" r="${r.toFixed(2)}" fill="rgb(${cr},${cg},${cb})" fill-opacity="${o.toFixed(2)}"/>`;
}
s+='</svg>'; writeFileSync(out,s); console.log(out, (s.length/1024|0)+'KB');
