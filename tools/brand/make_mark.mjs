// Writes the Couchlink mark as static SVG files: two links (a back ring hooked through a seat ring) on two legs.
// The over/under cuts sit exactly on the crossings of the two centre lines, so they are computed, not hand placed.
//
//   node tools/brand/make_mark.mjs            -> assets/brand/couchlink-mark.svg (Ocean) and couchlink-mark-mono.svg
import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const OUT = join(ROOT, 'assets', 'brand');

// Geometry on a 100 unit grid ("Original, cleaned"). Rects are centre lines; r is the corner radius.
const SW = 10;          // stroke width of both rings
const GAP = 2.5;        // clear gap each side of the strand that passes over
const BACK = { x: 24, y: 21, w: 52, h: 36, r: 18 };
const SEAT = { x: 11, y: 42, w: 78, h: 30, r: 15 };
const LEGS = [28, 72];  // leg centres
const LEG_W = SW - 1, LEG_H = 15;
const OCEAN = ['#5ac8fa', '#0a5cff']; // top left to bottom right

function sample(s, n = 1200) {
  const { x, y, w, h, r } = s, pts = [], segs = [];
  const line = (x1, y1, x2, y2) => segs.push([t => [x1 + (x2 - x1) * t, y1 + (y2 - y1) * t], Math.hypot(x2 - x1, y2 - y1)]);
  const arc = (cx, cy, a0) => segs.push([t => { const a = a0 + t * Math.PI / 2; return [cx + r * Math.cos(a), cy + r * Math.sin(a)]; }, Math.PI * r / 2]);
  line(x + r, y, x + w - r, y); arc(x + w - r, y + r, -Math.PI / 2);
  line(x + w, y + r, x + w, y + h - r); arc(x + w - r, y + h - r, 0);
  line(x + w - r, y + h, x + r, y + h); arc(x + r, y + h - r, Math.PI / 2);
  line(x, y + h - r, x, y + r); arc(x + r, y + r, Math.PI);
  const total = segs.reduce((a, [, l]) => a + l, 0);
  for (const [f, l] of segs) { const m = Math.max(1, Math.round(n * l / total)); for (let i = 0; i < m; i++) pts.push(f(i / m)); }
  return pts;
}

function crossings(A, B) {
  const out = [];
  for (let i = 0; i < A.length; i++) for (let j = 0; j < B.length; j++) {
    const [p1, p2, p3, p4] = [A[i], A[(i + 1) % A.length], B[j], B[(j + 1) % B.length]];
    const d = (p2[0] - p1[0]) * (p4[1] - p3[1]) - (p2[1] - p1[1]) * (p4[0] - p3[0]);
    if (Math.abs(d) < 1e-12) continue;
    const t = ((p3[0] - p1[0]) * (p4[1] - p3[1]) - (p3[1] - p1[1]) * (p4[0] - p3[0])) / d;
    const u = ((p3[0] - p1[0]) * (p2[1] - p1[1]) - (p3[1] - p1[1]) * (p2[0] - p1[0])) / d;
    if (t < 0 || t > 1 || u < 0 || u > 1) continue;
    const p = [p1[0] + t * (p2[0] - p1[0]), p1[1] + t * (p2[1] - p1[1])];
    if (!out.some(q => Math.hypot(q[0] - p[0], q[1] - p[1]) < 1)) out.push(p);
  }
  return out.sort((p, q) => p[0] - q[0]);
}

const f = v => +v.toFixed(3);
const rect = s => `x="${s.x}" y="${s.y}" width="${s.w}" height="${s.h}" rx="${s.r}"`;
const [L, R] = crossings(sample(BACK), sample(SEAT));
if (!L || !R) throw new Error('the rings do not cross twice');
const RC = SW * 1.5;

// Left crossing: the seat passes over the back. Right crossing: the back passes over the seat.
function svg(paint, defs = '') {
  const legY = SEAT.y + SEAT.h + SW / 2 - 3;
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 2.5 100 100" width="100" height="100">
  <defs>${defs}
    <clipPath id="cl"><circle cx="${f(L[0])}" cy="${f(L[1])}" r="${RC}"/></clipPath>
    <clipPath id="cr"><circle cx="${f(R[0])}" cy="${f(R[1])}" r="${RC}"/></clipPath>
    <mask id="mb" maskUnits="userSpaceOnUse" x="0" y="0" width="100" height="110">
      <rect width="100" height="110" fill="#fff"/>
      <rect ${rect(SEAT)} fill="none" stroke="#000" stroke-width="${SW + GAP * 2}" clip-path="url(#cl)"/>
    </mask>
    <mask id="ms" maskUnits="userSpaceOnUse" x="0" y="0" width="100" height="110">
      <rect width="100" height="110" fill="#fff"/>
      <rect ${rect(BACK)} fill="none" stroke="#000" stroke-width="${SW + GAP * 2}" clip-path="url(#cr)"/>
    </mask>
  </defs>
  ${LEGS.map(x => `<rect x="${x - LEG_W / 2}" y="${legY}" width="${LEG_W}" height="${LEG_H}" rx="${LEG_W / 2}" fill="${paint}"/>`).join('\n  ')}
  <rect ${rect(BACK)} fill="none" stroke="${paint}" stroke-width="${SW}" mask="url(#mb)"/>
  <rect ${rect(SEAT)} fill="none" stroke="${paint}" stroke-width="${SW}" mask="url(#ms)"/>
</svg>
`;
}

mkdirSync(OUT, { recursive: true });
const grad = `
    <linearGradient id="ocean" gradientUnits="userSpaceOnUse" x1="20" y1="10" x2="80" y2="90">
      <stop offset="0" stop-color="${OCEAN[0]}"/><stop offset="1" stop-color="${OCEAN[1]}"/>
    </linearGradient>`;
writeFileSync(join(OUT, 'couchlink-mark.svg'), svg('url(#ocean)', grad));
writeFileSync(join(OUT, 'couchlink-mark-mono.svg'), svg('currentColor'));
console.log(`crossings: left ${L.map(f)}, right ${R.map(f)}; wrote ${OUT}`);
