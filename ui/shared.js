// Outils partagés entre la scène et la lumière.
const T = window.__TAURI__;
const invoke = T.core.invoke;
const listen = (name, cb) => T.event.listen(name, (e) => cb(e.payload));

const WARM = ['#ff9a4d', '#ff6a3d', '#ffc27a', '#ff8a5c'];
const hsl = (h, s, l) => `hsl(${Math.round(h)} ${Math.round(s * 100)}% ${Math.round(l * 100)}%)`;
const norm = (s) => (s || '').toLowerCase().replace(/\s*[\(\[].*$/, '').replace(/\s+-\s+.*$/, '').trim();

// Tire de la pochette une couleur dominante et une seconde teinte (pas d'arc-en-ciel).
async function extractPalette(src) {
  if (!src) return WARM;
  try {
    const img = new Image();
    if (!src.startsWith('data:')) img.crossOrigin = 'anonymous';
    img.src = src; await img.decode();
    const n = 40, c = document.createElement('canvas'); c.width = c.height = n;
    const g = c.getContext('2d', { willReadFrequently: true }); g.drawImage(img, 0, 0, n, n);
    const d = g.getImageData(0, 0, n, n).data;
    const bins = Array.from({ length: 12 }, () => ({ w: 0, x: 0, y: 0, s: 0 }));
    for (let i = 0; i < d.length; i += 4) {
      const r = d[i] / 255, gg = d[i + 1] / 255, b = d[i + 2] / 255;
      const mx = Math.max(r, gg, b), mn = Math.min(r, gg, b), dl = mx - mn;
      const s = mx ? dl / mx : 0;
      if (s < 0.22 || mx < 0.22) continue;
      let h = mx === r ? ((gg - b) / dl) % 6 : mx === gg ? (b - r) / dl + 2 : (r - gg) / dl + 4;
      h = (h * 60 + 360) % 360;
      const w = s * mx, bin = bins[Math.floor(h / 30) % 12], a = h * Math.PI / 180;
      bin.w += w; bin.x += Math.cos(a) * w; bin.y += Math.sin(a) * w; bin.s += s * w;
    }
    const top = bins.filter(b => b.w > 4).sort((a, b) => b.w - a.w).slice(0, 2)
      .map(b => ({ h: (Math.atan2(b.y, b.x) * 180 / Math.PI + 360) % 360, s: b.s / b.w }));
    if (!top.length) return ['#f4f1ff', '#cfd6ff', '#ffffff', '#e6dcff'];   // pochette noir et blanc
    const a = top[0], b = top[1] || { h: (a.h + 24) % 360, s: a.s };
    const sa = Math.min(0.95, Math.max(0.7, a.s)), sb = Math.min(0.95, Math.max(0.7, b.s));
    return [hsl(a.h, sa, 0.6), hsl(b.h, sb, 0.58), hsl((a.h + 14) % 360, sa, 0.72), hsl((a.h + 346) % 360, sa, 0.52)];
  } catch { return WARM; }
}

function nowPos(st) { return st && st.playing ? st.pos + (Date.now() - st.at) : (st ? st.pos : 0); }

async function getJson(url) {
  try { return JSON.parse(await invoke('http_get', { url })); } catch { return null; }
}
