(() => {
'use strict';
const FONT = '"Segoe UI Variable Text","Segoe UI",system-ui,-apple-system,"Malgun Gothic","Apple SD Gothic Neo",sans-serif';
const qs = new URLSearchParams(location.search);
const FREEZE = qs.has('t') ? parseFloat(qs.get('t')) : null;   // ?t=2.4 holds one frame of the loop
// The loop is the page's content, like an embedded video, so it plays even
// where the system asks for reduced motion.
const STILL = FREEZE !== null;

const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
const ease = t => (t = clamp(t, 0, 1), t * t * (3 - 2 * t));
const follow = (cur, to, dt, tau) => to + (cur - to) * Math.exp(-dt / tau);
const rgba = (r, g, b, a) => `rgba(${Math.round(r * 255)},${Math.round(g * 255)},${Math.round(b * 255)},${a})`;
/* The app's colours (src/paint.rs, src/board.rs). */
const white = a => rgba(1, 1, 1, a);
const accent = a => rgba(.34, .62, 1, a);
/* Pins' light, as the app draws it: the one blue, pooling below the window, deeper toward its foot, and faint round its edge.
   Pinned to every shelf, twice the reach and the strength; to one shelf, smaller and fainter, still seen at a glance. */
const PIN_LIGHT = { all: { reach: 2, a: 1 }, row: { reach: 1, a: .45 } };
const pinColour = (kind, a) => accent(clamp(PIN_LIGHT[kind].a * a, 0, 1));
const BACKDROP = rgba(.050, .055, .068, 1);
/* The desktop strip (src/follow.rs): GAP between desktops, the others DIM darker. */
const GAP = .07, DIM = .55;
const scaleAt = depth => 1 - .5 * clamp(depth, 0, 1);
const standingOut = (emphasis, receded) => {
  emphasis = clamp(emphasis, 0, 1); receded = clamp(receded, 0, 1);
  return [(1 - (.65 - .15 * emphasis) * receded) / scaleAt(receded), DIM * (1 - emphasis)];
};

/* ---------- the page's language: its shelves' names and its own pictures ---------- */
const PAGE = window.FLICK;

/* ---------- drawing helpers ---------- */
function rr(g, x, y, w, h, r) {
  r = Math.max(0, Math.min(r, w / 2, h / 2));
  g.beginPath(); g.moveTo(x + r, y);
  g.arcTo(x + w, y, x + w, y + h, r); g.arcTo(x + w, y + h, x, y + h, r);
  g.arcTo(x, y + h, x, y, r); g.arcTo(x, y, x + w, y, r); g.closePath();
}
function fill(g, x, y, w, h, r, c) { g.fillStyle = c; rr(g, x, y, w, h, r); g.fill(); }
function txt(g, s, x, y, px, c, weight, align) {
  g.font = `${weight || 400} ${px}px ${FONT}`; g.fillStyle = c; g.textAlign = align || 'left'; g.textBaseline = 'middle'; g.fillText(s, x, y);
}
/* A soft glow of `colour` round a rounded rectangle, as paint::Glow draws it: the shape's blurred shadow, the shape itself kept out of sight. */
function glow(g, x, y, w, h, r, blur, colour) {
  const far = 10000, k = g.getTransform().a;   // shadows are measured in device pixels, untransformed
  g.save(); g.shadowColor = colour; g.shadowBlur = blur * k; g.shadowOffsetX = far * k;
  fill(g, x - far, y, w, h, r, '#000'); g.restore();
}
/* A pinned window's light round x, y, w, h: `a` fades it, 1 at full. */
function pinLight(g, x, y, w, h, s, kind, a) {
  const reach = PIN_LIGHT[kind].reach, c = v => pinColour(kind, v * a);
  /* The pool: brightest at the window's foot, spreading down and out. */
  g.save(); g.globalCompositeOperation = 'screen';
  g.translate(x + w / 2, y + h * .8); g.scale(w * (.4 + .3 * reach), (h * .25 + 36 * s) * reach);
  const pool = g.createRadialGradient(0, 0, 0, 0, 0, 1);
  pool.addColorStop(0, c(1)); pool.addColorStop(.4, c(.5)); pool.addColorStop(1, c(0));
  g.fillStyle = pool; g.beginPath(); g.arc(0, 0, 1, 0, 7); g.fill(); g.restore();
  glow(g, x - 2 * s, y - 2 * s, w + 4 * s, h + 4 * s, 10 * s, 18 * reach * s, c(.8));   // the rim
}
/* board.rs `surface`: a face standing off the dark, its soft shadow higher the more it is lifted, and a faint line of light along its top. */
function surface(g, x, y, w, h, r, face, alpha, lift, s) {
  glow(g, x, y + (5 + 5 * lift) * s, w, h, r, (34 + 26 * lift) * s * .5, `rgba(0,0,0,${(.36 + .24 * lift) * alpha})`);
  glow(g, x, y + s, w, h, r, (8 + 4 * lift) * s * .5, `rgba(0,0,0,${.5 * alpha})`);
  fill(g, x, y, w, h, r, rgba(face, face + .006, face + .02, alpha));
  const gr = g.createLinearGradient(0, y, 0, y + h); gr.addColorStop(0, white(.055 * alpha)); gr.addColorStop(.5, white(0));
  fill(g, x, y, w, h, r, gr);
  const inset = Math.max(r, 2 * s);
  if (w > 2 * inset) { g.fillStyle = white(.07 * alpha); g.fillRect(x + inset, y + .25 * s, w - 2 * inset, s); }
}

/* ---------- the desktops: real Windows 11 screens (scripts/landing-cells.ps1), one shelf per piece of work ---------- */
const CELLS = PAGE.cells;   // docs/cells/<lang>/cells.js, made by scripts/landing-cells.py
const shelfName = r => PAGE.shelves[r];
const SHELVES = CELLS.shelves;
const PINNED_ALL = CELLS.pinAll, PINNED_ROW = CELLS.pinRow;
/* An app's colour, for its icon until the picture of it has come. */
const TONE = { notepad: '#3d8bfd', terminal: '#8d96a8', edge: '#2fa8d8', explorer: '#e6c34a', paint: '#e46a9a', calc: '#8f7cf0', app: '#2fa8d8' };

const IMG = {};
let onImage = () => {};
function image(path) {
  let im = IMG[path];
  if (!im) { im = IMG[path] = new Image(); im.decoding = 'async'; im.onload = () => onImage(); im.src = PAGE.base + path; }
  return im.complete && im.naturalWidth ? im : null;
}
/* An app's icon: its own picture, or a rounded square of its colour while that loads. */
function icon(g, app, x, y, k) {
  const im = image(`icons/${app}.png`);
  if (im) { g.imageSmoothingQuality = 'high'; g.drawImage(im, x, y, k, k); return; }
  fill(g, x, y, k, k, k * .24, TONE[app]);
}

/* ---------- the loop: a developer who also markets, building Flick 0.6 above, launching it below, a break between ----------
   How far the screens recede follows the hand: a sure, quick flick slides them full size, a middling one
   to three quarters, a slow or unsure drag all the way back. The video on the break shelf plays on while the
   person works: each time they flick down to it, it is further along, and each time they flick back up their
   work is as they left it. The team's chat is pinned to both shelves of work and is gone on the break. */
const SCRIPT = [
  [.4, h => h.drag(1, 0)],                       // building: the source, then the release on GitHub
  [2.0, h => h.drag(0, 1)],                      // one flick down: the video, playing
  [3.7, h => h.drag(0, -1)],                     // one flick up: the release, as it was
  [5.2, h => h.press()],                         // held, the shelves recede: on to the project folder
  [5.6, h => h.drag(1, 0)],
  [6.3, h => h.release()],
  [7.6, h => h.drag(0, 1)],                      // down again: the video has gone on
  [9.3, h => h.drag(0, -1)],                     // and back to work
  [10.8, h => h.press(.5)],                      // a middling flick: three quarters size
  [11.0, h => h.drag(0, 1)],
  [11.6, h => h.drag(1, 0)],                     // music next to the video
  [12.2, h => h.drag(-1, 0)],
  [12.7, h => h.release()],
  [14.0, h => h.press()],
  [14.4, h => h.drag(0, 1)],                     // down to marketing: the chat comes back with it
  [15.1, h => h.drag(1, 0)],                     // the desktops slide; the chat stays
  [15.8, h => h.drag(1, 0)],
  [16.5, h => h.release()],
  [17.8, h => h.drag(-1, 0)],                    // a quick flick back to the numbers
  [19.0, h => h.tap()],                          // the board: what is pinned where
  [21.6, h => h.tap()],
  [22.4, h => h.drag(0, -1)],                    // up to the break: further along still
  [24.0, h => h.drag(0, 1)],                     // down: the launch work, as it was
  [25.5, h => h.press()],
  [25.9, h => h.drag(0, -1)],                    // up through the break to building, and along it home
  [26.4, h => h.drag(0, -1)],
  [27.0, h => h.drag(-1, 0)],
  [27.5, h => h.drag(-1, 0)],
  [28.0, h => h.release()],
];
const LOOP = 29.8;
/* Which picture of the playing video each visit to the break shelf finds. */
const VIDEO_AT = [0, 1, 1, 2, 2];
/* Springs, a little under-damped: quick to go, a few per cent past, quick to settle. */
const MOVE = { w: 24, z: .74 }, DEPTH = { w: 22, z: .7 }, BOARD = { w: 20, z: .72 };
function spring(s, to, dt, { w, z }) {
  for (let n = Math.ceil(dt / (1 / 240)), h = dt / n; n > 0; n--) { s.v += (-w * w * (s.p - to) - 2 * z * w * s.v) * h; s.p += s.v * h; }
}

class Hero {
  constructor(cv) {
    this.cv = cv; this.g = cv.getContext('2d'); this.visible = true; this.quiet = false;
    this.onBeat = () => {};
    this.reset(); this.resize();
  }
  reset() {
    this.t = 0; this.si = 0;
    this.row = 0; this.col = 0; this.last = SHELVES.map(() => 0);
    this.sx = { p: 0, v: 0 }; this.sy = { p: 0, v: 0 }; this.tx = 0; this.ty = 0;   // where the strip is: column on the current shelf, and shelf
    this.held = false; this.depth = { p: 0, v: 0 }; this.arrived = -9; this.breaks = 0;   // when a shelf with the chat was last come to; visits to the break
    this.board = { open: false, s: { p: 0, v: 0 } };
  }
  get x() { return this.sx.p; }
  get y() { return this.sy.p; }
  get receded() { return this.depth.p; }
  resize() {
    const r = this.cv.getBoundingClientRect(), dpr = Math.min(window.devicePixelRatio || 1, 2);
    this.W = Math.max(1, Math.round(r.width)); this.H = Math.max(1, Math.round(r.height)); this.dpr = dpr;
    this.cv.width = Math.round(this.W * dpr); this.cv.height = Math.round(this.H * dpr);
    this.u = this.H / 620;                       // one of the app's pixels on this stage
    this.tu = Math.max(this.u, .62);             // the same for text, kept legible on a phone
    this.narrow = this.W < 560;
    /* The screen fills the stage, cut at its sides if the stage is narrower. */
    this.sc = Math.max(this.W / CELLS.sw, this.H / CELLS.sh);
    this.ox = (this.W - CELLS.sw * this.sc) / 2; this.oy = (this.H - CELLS.sh * this.sc) / 2;
  }
  beat(i) { if (!this.quiet) this.onBeat(i); }
  press(depth = 1) { this.held = true; this.deep = depth; this.beat(0); }
  release() { this.held = false; this.beat(2); }
  drag(dx, dy) {
    this.tx = clamp(this.col + dx, 0, SHELVES[this.row].length - 1);
    this.ty = clamp(this.row + dy, 0, SHELVES.length - 1);
    this.beat(1);
  }
  tap() { this.board.open = !this.board.open; }

  step(dt) {
    this.t += dt;
    while (this.si < SCRIPT.length && SCRIPT[this.si][0] <= this.t) SCRIPT[this.si++][1](this);
    if (this.t >= LOOP) this.reset();
    /* The strip springs to where the hand sent it; the desktop it is nearest is the one chosen. */
    spring(this.sy, this.ty, dt, MOVE);
    const row = clamp(Math.round(this.y), 0, SHELVES.length - 1);
    if (row !== this.row) { if (PINNED_ROW.rows.includes(row) && !PINNED_ROW.rows.includes(this.row)) this.arrived = this.t; if (SHELVES[row].some(d => d.frames)) this.breaks++; this.last[this.row] = this.col; this.row = row; this.col = this.last[row]; this.tx = this.col; this.sx = { p: this.col, v: 0 }; }
    spring(this.sx, this.tx, dt, MOVE);
    this.col = clamp(Math.round(this.x), 0, SHELVES[this.row].length - 1);
    /* Held, the screen recedes; let go, the chosen desktop comes forward. */
    spring(this.depth, this.held ? this.deep : 0, dt, DEPTH);
    spring(this.board.s, this.board.open ? 1 : 0, dt, BOARD);
  }
  seek(t) { this.quiet = true; this.reset(); for (let i = 0, n = Math.round(t * 60); i < n; i++) this.step(1 / 60); this.quiet = false; }

  /* A rectangle of the real screen, in the stage's pixels. */
  onStage(r) { const { sc, ox, oy } = this; return [ox + r[0] * sc, oy + r[1] * sc, r[2] * sc, r[3] * sc]; }
  /* A window by itself: its shadow, then its picture with Windows 11's 8px corners. Before the picture comes, its desktop's. */
  paintShot(g, win, desk, x, y, w, h, lift) {
    const s = w / win.r[2], r = 8 * s;   // one of the screen's pixels, here
    glow(g, x, y + (6 + 8 * lift) * s, w, h, r, (20 + 16 * lift) * s, `rgba(0,0,0,${.32 + .2 * lift})`);
    glow(g, x, y + s, w, h, r, 4 * s, 'rgba(0,0,0,.4)');
    const own = win.img && image(win.img), from = own || image(this.picture(desk));
    g.save(); rr(g, x, y, w, h, r); g.clip();
    if (from) {
      const k = from.naturalWidth / (own ? win.r[2] : CELLS.sw);
      if (own) g.drawImage(own, x, y, w, h);
      else g.drawImage(from, win.r[0] * k, win.r[1] * k, win.r[2] * k, win.r[3] * k, x, y, w, h);
    } else { g.fillStyle = '#202328'; g.fillRect(x, y, w, h); }
    g.restore();
  }
  /* A desktop's picture; the playing video's, as far along as this visit finds it. */
  picture(desk) { return desk.frames ? desk.frames[Math.min(desk.frames.length - 1, VIDEO_AT[Math.max(0, this.breaks - 1)] || 0)] : desk.img; }
  /* follow::placed: where a thing `apart` desktops from the strip is drawn, and how dark. */
  place(ax, ay) {
    const { W, H } = this, sc = scaleAt(this.receded), emphasis = clamp(1 - Math.hypot(ax, ay), 0, 1);
    const [lift, dark] = standingOut(emphasis, this.receded), k = sc * lift;
    const cx = W / 2 + ax * W * (1 + GAP) * sc, cy = H / 2 + ay * H * (1 + GAP) * sc;
    return { x: cx - W * k / 2, y: cy - H * k / 2, k, dark: this.receded > .001 ? clamp(dark, 0, 1) : 0 };
  }
  /* Where shelf `r` stands sideways: the shelf the hand is on goes with the strip, the others wait on the desktop they were left on (follow::step_along). */
  rowX(r) {
    const off = this.x - this.col;
    return (r === this.row ? this.col : this.last[r]) + off * clamp(1 - Math.abs(r - this.y), 0, 1);
  }
  drawGlide() {
    const g = this.g, { W, H, sc, ox, oy } = this;
    g.fillStyle = BACKDROP; g.fillRect(0, 0, W, H);
    SHELVES.forEach((row, r) => row.forEach((desk, c) => {
      const p = this.place(c - this.rowX(r), r - this.y), w = W * p.k, h = H * p.k;
      if (p.x > W || p.y > H || p.x + w < 0 || p.y + h < 0) return;
      const im = image(this.picture(desk));
      if (im) {
        const k = im.naturalWidth / CELLS.sw;
        g.drawImage(im, -ox / sc * k, -oy / sc * k, W / sc * k, H / sc * k, p.x, p.y, w, h);
      } else { g.fillStyle = '#1e2127'; g.fillRect(p.x, p.y, w, h); }
      if (p.dark > 0) { g.fillStyle = `rgba(0,0,0,${p.dark})`; g.fillRect(p.x, p.y, w, h); }
    }));
    /* Pinned windows stay where they are sideways; those of a shelf go up and down with it. */
    /* Each with its pin's light round it, no box: this shelf's brighter as the shelf is come to. */
    /* They stay where they are; receding, one covering much of the screen shrinks about its own middle, to no less than 40%.
       A shelf's own go up and down with it. */
    const pinned = (win, ay, kind, a) => {
      const d = clamp(this.receded, 0, 1), [rx, ry, rw, rh] = this.onStage(win.r);
      const covers = win.r[2] * win.r[3] / (CELLS.sw * CELLS.sh), k = 1 - d * (1 - clamp(Math.sqrt(.03 / covers), .4, 1));
      const w = rw * k, h = rh * k, x = rx + (rw - w) / 2, y = ry + (rh - h) / 2 + ay * H * (1 + GAP) * scaleAt(this.receded);
      const s = sc * Math.max(k, .5);   // the light shrinks less than the window, to stay seen
      pinLight(g, x, y, w, h, s, kind, a * clamp(1 - Math.abs(ay), 0, 1));
      this.paintShot(g, win, win, x, y, w, h, 1);
    };
    PINNED_ROW.rows.forEach(r => { if (Math.abs(r - this.y) < 1.5) pinned(PINNED_ROW, r - this.y, 'row', 1 + .4 * Math.exp(-(this.t - this.arrived) / 1.2)); });
    pinned(PINNED_ALL, 0, 'all', 1);
    if (this.receded > .01) this.drawBanner();
  }
  /* glide.rs Banner: the shelf's name in a pill above the picture, the names sliding as the shelf changes. */
  drawBanner() {
    const g = this.g, s = this.tu, W = this.W, h = 30 * s, top = 14 * s, a = clamp(this.receded, 0, 1);
    g.font = `400 ${13 * s}px ${FONT}`;
    const shown = SHELVES.map((_, r) => [r, 1 - Math.abs(r - this.y)]).filter(([, k]) => k > 0);
    const wide = clamp(shown.reduce((sum, [r, k]) => sum + (g.measureText(shelfName(r)).width + 28 * s) * k, 0), h * 2, W - 2);
    const x = (W - wide) / 2;
    g.save(); g.globalAlpha = a;
    fill(g, x, top, wide, h, h / 2, rgba(.06, .065, .08, .56));
    g.strokeStyle = white(.10); g.lineWidth = 1; rr(g, x + .5, top + .5, wide - 1, h - 1, h / 2); g.stroke();
    g.save(); rr(g, x, top, wide, h, h / 2); g.clip();
    shown.forEach(([r, k]) => txt(g, shelfName(r), W / 2, top + h / 2 + (r - this.y) * h, 13 * s, white(.92 * k), 400, 'center'));
    g.restore(); g.restore();
  }

  /* ---------- the board (src/board.rs) ---------- */
  boardLayout() {
    const { W, H, u: s, narrow } = this, n = SHELVES.length;
    /* The map: squares 0.62 as tall as wide, the shelves shifted so the desktop each was left on is in one column. */
    const ch = (narrow ? 30 : 46) * s, cw = ch / .62, gap = 8 * s, pitch = cw + gap, rp = ch + gap;
    const anchor = r => r === this.row ? this.col : this.last[r];
    const lead = Math.max(...SHELVES.map((_, r) => anchor(r))), tail = Math.max(...SHELVES.map((row, r) => row.length - anchor(r)));
    const nameW = (narrow ? 70 : 84) * s, mapW = nameW + (lead + tail) * pitch - gap, mapTop = H - n * rp - 30 * s;
    const x0 = (W - mapW) / 2, spineX = x0 + nameW + lead * pitch;
    const cells = SHELVES.map((row, r) => row.map((desk, c) => ({ x: spineX + (c - anchor(r)) * pitch, y: mapTop + r * rp, w: cw, h: ch, wins: desk.wins, r, c })));
    /* Pinned windows at the left, all shelves above this shelf; the chosen desktop's windows as large previews beside them. */
    const top = 58 * s, bottom = mapTop - 34 * s, colW = W * .19, left = 30 * s, slots = this.pinSlots();
    const pins = [
      { win: PINNED_ALL, kind: 'all', ...this.fitTile(PINNED_ALL, ...slots.all) },
      ...(PINNED_ROW.rows.includes(this.row) ? [{ win: PINNED_ROW, kind: 'row', ...this.fitTile(PINNED_ROW, ...slots.row) }] : []),
    ];
    const desk = SHELVES[this.row][this.col], wins = desk.wins, ax = left + colW + 40 * s, aw = W - ax - 40 * s, tg = 26 * s;
    const asp = wins.map(w => w.r[2] / w.r[3]), sum = asp.reduce((a, b) => a + b, 0);
    const ph = Math.min(bottom - top - 30 * s, (aw - tg * (wins.length - 1)) / sum);
    let tx = ax + (aw - (sum * ph + tg * (wins.length - 1))) / 2; const ty = top + (bottom - top - ph - 30 * s) / 2;
    const tiles = wins.map((win, i) => { const t = { win, desk, x: tx, y: ty, w: asp[i] * ph, h: ph }; tx += t.w + tg; return t; });
    return { cells, tiles, pins, ch, gap, x0, right: x0 + mapW, spine: { x: spineX - gap / 2, y: mapTop - gap / 2, w: pitch, h: n * rp } };
  }
  /* Where the board keeps pinned windows: at its left, those on every shelf above those on this shelf. */
  pinSlots() {
    const { H, W, u: s, narrow } = this, ch = (narrow ? 30 : 46) * s, rp = ch + 8 * s;
    const top = 58 * s, bottom = H - SHELVES.length * rp - 30 * s - 34 * s, colW = W * .19, left = 30 * s, pinH = (bottom - top - 24 * s) / 2;
    return { all: [left, top, colW, pinH], row: [left, top + pinH + 24 * s, colW, pinH] };
  }
  fitTile(win, x, y, w, h) {
    const asp = win.r[2] / win.r[3], th = Math.min(h - 30 * this.u, w / asp), tw = th * asp;
    return { x: x + (w - tw) / 2, y: y + (h - 30 * this.u - th) / 2, w: tw, h: th };
  }
  /* A preview: the glow of its pin under it, a shrunken picture of the window, its app's icon over its lower left corner. */
  drawTile(t, pin) {
    const g = this.g, s = this.u, win = t.win;
    if (pin) pinLight(g, t.x, t.y, t.w, t.h, .6 * s, pin, 1);
    this.paintShot(g, win, t.desk || win, t.x, t.y, t.w, t.h, .5);
    const side = 32 * s; icon(g, win.app, t.x - 6 * s, t.y + t.h - side + 10 * s, side);
  }
  drawBoard() {
    const g = this.g, { W, H, u: s, tu, dpr } = this, L = this.boardLayout(), a = clamp(this.board.s.p, 0, 1);
    g.save(); g.globalAlpha = a;
    g.fillStyle = BACKDROP; g.fillRect(0, 0, W, H);
    /* Where pinned windows go: a pool of their colour on the floor of each place. */
    L.pins.forEach(p => glow(g, p.x + p.w * .12, p.y + p.h + 14 * s, p.w * .76, 6 * s, 3 * s, 70 * s * .5, pinColour(p.kind, .2)));
    const z = .96 + .04 * this.board.s.p;
    g.translate(W / 2, H / 2); g.scale(z, z); g.translate(-W / 2, -H / 2);
    L.pins.forEach(p => this.drawTile(p, p.kind));
    L.tiles.forEach(t => this.drawTile(t, null));
    g.setTransform(dpr, 0, 0, dpr, 0, 0); g.globalAlpha = a;
    /* The shelf's name at the top. */
    txt(g, shelfName(this.row), W / 2, 18 * s + 15 * tu, 13.5 * tu, white(.86), 600, 'center');
    /* The map: the column vertical moves travel along, the shelves' names, the cells with their apps' icons, the chosen one raised. */
    fill(g, L.spine.x, L.spine.y, L.spine.w, L.spine.h, 12 * s, 'rgba(0,0,0,.16)');
    /* Windows on every desktop: a thin line of their colour over the map. */
    const ly = L.spine.y - 6 * s, gr = g.createLinearGradient(L.x0, 0, L.right, 0);
    gr.addColorStop(0, accent(0)); gr.addColorStop(.5, accent(.65)); gr.addColorStop(1, accent(0));
    g.fillStyle = gr; g.fillRect(L.x0, ly, L.right - L.x0, 1.2 * s);
    L.cells.forEach((row, r) => txt(g, PAGE.short[r], L.x0, row[0].y + L.ch / 2, 12 * tu, white(r === this.row ? .86 : .5), r === this.row ? 600 : 400));
    L.cells.forEach(row => row.forEach(c => {
      const chosen = c.r === this.row && c.c === this.col;
      if (chosen) {
        glow(g, c.x - 3 * s, c.y - 3 * s, c.w + 6 * s, c.h + 6 * s, 11 * s, 15 * s, rgba(.80, .86, 1, .14));
        surface(g, c.x - 3 * s, c.y - 3 * s, c.w + 6 * s, c.h + 6 * s, 11 * s, .215, 1, 1, s);
      } else surface(g, c.x, c.y, c.w, c.h, 9 * s, .10 + .075 * Math.min(1, c.wins.length / 3) + (c.r === this.row ? .012 : 0), 1, 0, s);
      if (chosen) { glow(g, c.x + 6.5 * s, c.y + 6.5 * s, 3 * s, 3 * s, 1.5 * s, 5 * s, accent(.5)); g.fillStyle = accent(1); g.beginPath(); g.arc(c.x + 8 * s, c.y + 8 * s, 2.6 * s, 0, 7); g.fill(); }
      const k = Math.min(16 * s, c.h * .36), n = c.wins.length, x = c.x + (c.w - (n * k + (n - 1) * 4 * s)) / 2;
      c.wins.forEach((win, i) => icon(g, win.app, x + i * (k + 4 * s), c.y + (c.h - k) / 2, k));
    }));
    g.restore();
  }

  draw() {
    const g = this.g, a = this.board.s.p;
    g.setTransform(this.dpr, 0, 0, this.dpr, 0, 0); g.globalAlpha = 1;
    if (a < .995) this.drawGlide();
    if (a > .005) this.drawBoard();
  }
}

/* ---------- wiring ---------- */
const hero = new Hero(document.getElementById('hero'));
/* The pictures come in their own time; a held frame is drawn again as each arrives. */
onImage = () => { if (STILL) redrawStill(); };
SHELVES.forEach(row => row.forEach(d => { image(d.img); (d.frames || []).forEach(image); d.wins.forEach(w => { image(`icons/${w.app}.png`); if (w.img) image(w.img); }); }));
[PINNED_ALL, PINNED_ROW].forEach(w => { image(w.img); image(`icons/${w.app}.png`); });
const words = [...document.querySelectorAll('#line span')];
let beatTimer = 0;
hero.onBeat = i => { words.forEach((w, j) => w.classList.toggle('on', j === i)); clearTimeout(beatTimer); beatTimer = setTimeout(() => words.forEach(w => w.classList.remove('on')), 650); };

function redrawStill() { hero.seek(FREEZE); hero.draw(); }
new ResizeObserver(() => { hero.resize(); if (STILL) redrawStill(); else hero.draw(); }).observe(hero.cv);

/* Plays by itself, and only while it is on screen. */
if ('IntersectionObserver' in window) { hero.visible = false; new IntersectionObserver(es => { hero.visible = es[es.length - 1].isIntersecting; kick(); }, { threshold: .05 }).observe(hero.cv); }

let raf = 0, last = 0;
function frame(now) {
  raf = 0;
  const dt = Math.min(.05, (now - last) / 1000 || 0); last = now;
  if (!hero.visible || document.hidden) return;
  hero.step(dt); hero.draw(); raf = requestAnimationFrame(frame);
}
function kick() { if (STILL || raf || document.hidden || !hero.visible) return; last = performance.now(); raf = requestAnimationFrame(frame); }
document.addEventListener('visibilitychange', kick);
if (STILL) redrawStill(); else kick();
})();
