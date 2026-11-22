//! The two pages served to phones, embedded in the program (nothing to deploy).
//! Styled after the brand (docs/branding/ICON.md): dark neutral ground, white ink, the
//! orange accent for what's live and what advances; the logo and arrow are inlined.

pub fn login(message: Option<&str>) -> String {
    let msg = message
        .map(|m| format!("<p class=\"err\">{}</p>", escape(m)))
        .unwrap_or_default();
    format!(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1">
<title>OpenPresenter Remote</title>
<style>
body {{ font-family: system-ui, sans-serif; background: linear-gradient(#2A2A31, #121215); background-color: #121215; color: #eee; margin: 0; display: flex; min-height: 100vh; align-items: center; justify-content: center; }}
form {{ background: #1e1e24; padding: 24px; border-radius: 12px; width: min(90vw, 320px); }}
h1 {{ font-size: 1.3rem; margin: 0 0 16px; display: flex; align-items: center; gap: 10px; }}
.logo {{ width: 36px; height: 36px; flex: none; }}
input {{ width: 100%; box-sizing: border-box; font-size: 2rem; letter-spacing: .4em; text-align: center; padding: 10px; border-radius: 8px; border: 1px solid #3a3a44; background: #121215; color: #fff; }}
button {{ width: 100%; margin-top: 16px; font-size: 1.2rem; padding: 14px; border: 0; border-radius: 8px; background: linear-gradient(135deg, #FFA845, #F06A1D); color: #fff; font-weight: 600; }}
.err {{ color: #ff7b7b; }}
p {{ color: #aaa; font-size: .9rem; }}
</style></head>
<body><form method="post" action="/login">
<h1><svg class="logo" viewBox="0 0 512 512" aria-hidden="true"><defs><linearGradient id="op-bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2A2A31"/><stop offset="1" stop-color="#121215"/></linearGradient><linearGradient id="op-accent" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#FFA845"/><stop offset="1" stop-color="#F06A1D"/></linearGradient></defs><rect width="512" height="512" rx="96" fill="url(#op-bg)"/><g transform="translate(0 6)"><rect x="226" y="320" width="60" height="70" fill="url(#op-accent)"/><rect x="150" y="364" width="212" height="56" rx="28" fill="url(#op-accent)"/><path d="M 280 110 H 180 A 72 72 0 0 0 108 182 V 252 A 72 72 0 0 0 180 324 H 332 A 72 72 0 0 0 404 252 V 246" fill="none" stroke="#FFFFFF" stroke-width="60" stroke-linecap="round"/><rect x="168" y="196" width="124" height="48" rx="24" fill="#FFFFFF"/><path d="M 358 88 L 424 154 L 358 220" fill="none" stroke="url(#op-accent)" stroke-width="56" stroke-linecap="round" stroke-linejoin="round"/></g></svg>OpenPresenter Remote</h1>
<p>Enter the PIN shown on the operator screen.</p>
{msg}
<input name="pin" inputmode="numeric" pattern="[0-9]*" autocomplete="one-time-code" maxlength="6" autofocus required>
<button type="submit">Connect</button>
</form></body></html>"##
    )
}

/// The remote. One page, two layouts: a phone gets current/next and the big
/// buttons; a wide screen also gets the playlists and a slide grid (click = live).
pub const REMOTE: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no">
<meta name="theme-color" content="#121215">
<title>OpenPresenter Remote</title>
<style>
:root { --bg: #121215; --panel: #1e1e24; --line: #2e2e36; --text: #eee; --dim: #9a9aa6; --live: #F88A31; --accent-start: #FFA845; --accent-end: #F06A1D; --next: #2a6df4; --danger: #8a2626; }
* { box-sizing: border-box; -webkit-tap-highlight-color: transparent; }
html, body { margin: 0; height: 100%; }
body { font-family: system-ui, sans-serif; background: var(--bg); color: var(--text); display: flex; flex-direction: column; height: 100vh; height: 100dvh; touch-action: manipulation; overflow: hidden; }
header { padding: 8px 16px; font-size: .9rem; color: var(--dim); display: flex; justify-content: space-between; gap: 12px; border-bottom: 1px solid var(--line); }
.brand { display: flex; align-items: center; gap: 8px; min-width: 0; }
.logo { width: 20px; height: 20px; flex: none; }
#title { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text); }
#status { white-space: nowrap; }
#status.off { color: #ff7b7b; }
#app { flex: 1; min-height: 0; display: flex; flex-direction: column; }
#lists, #browser, #extra { display: none; }
#stage { flex: 1; min-height: 0; display: flex; flex-direction: column; padding: 12px 16px; gap: 10px; overflow: auto; }
.label { font-size: .75rem; text-transform: uppercase; letter-spacing: .04em; color: var(--dim); display: flex; gap: 8px; align-items: center; min-height: 1.2em; }
.badge { background: #5a1f1f; color: #ffb3b3; border-radius: 4px; padding: 1px 6px; text-transform: none; letter-spacing: 0; }
.screen { position: relative; width: 100%; aspect-ratio: 16 / 9; background: #000; border-radius: 6px; overflow: hidden; }
.screen img { position: absolute; inset: 0; width: 100%; height: 100%; display: none; }
.screen img.bg { object-fit: cover; }
.screen img.sl { object-fit: contain; }
.screen img.on { display: block; }
.screen .fb { position: absolute; inset: 0; display: none; align-items: center; justify-content: center; text-align: center; white-space: pre-line; padding: 8px; font-size: 1.1rem; color: #ddd; }
.screen .fb.on { display: flex; }
#cur { border: 3px solid var(--live); }
#cur.cleared .fb { opacity: .35; }
.nextrow { display: grid; grid-template-columns: 1fr 1.3fr; gap: 12px; align-items: center; }
#nexttitle { font-size: .95rem; color: #ccc; overflow: hidden; text-overflow: ellipsis; }
#nxt .fb { font-size: .8rem; }
nav { display: grid; grid-template-columns: 1fr 2fr; gap: 10px; padding: 12px 16px calc(12px + env(safe-area-inset-bottom)); border-top: 1px solid var(--line); }
button { font: inherit; color: #fff; border: 0; border-radius: 12px; cursor: pointer; }
nav button { font-size: 1.4rem; padding: 22px 0; }
#prev { background: #333; }
#nextbtn { background: linear-gradient(135deg, var(--accent-start), var(--accent-end)); font-weight: 600; }
nav button { display: flex; align-items: center; justify-content: center; gap: 12px; }
.arrow { height: .9em; width: auto; }
.arrow.back { transform: scaleX(-1); }
#clear { grid-column: 1 / 3; background: var(--danger); padding: 14px 0; font-size: 1.1rem; }
#holds { grid-column: 1 / 3; display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
#holds button { background: #2a2a2a; padding: 14px 0; font-size: 1.1rem; border: 2px solid transparent; }
#freeze.on { background: #1d5a70; border-color: #40b0d6; }
#blank.on { background: #000; border-color: #fff; }
button:active { filter: brightness(1.3); }
button:focus-visible { outline: 2px solid #fff; outline-offset: 2px; }

/* Phone on its side: current and next side by side. */
@media (orientation: landscape) and (max-height: 500px) {
  #app { flex-direction: row; }
  #stage { flex-direction: row; align-items: flex-start; }
  #stage > .curbox { flex: 2; }
  #stage > .nextbox { flex: 1; }
  .nextrow { grid-template-columns: 1fr; }
  nav { grid-template-columns: 1fr; grid-template-rows: 2fr 3fr 1fr 1fr; width: 34%; border-top: 0; border-left: 1px solid var(--line); }
  nav button { padding: 0; }
  #clear, #holds { grid-column: auto; }
}

/* Wide screens: playlists | slide grid | current/next + controls. */
@media (min-width: 900px) and (min-height: 500px) {
  #app { display: grid; grid-template-columns: 240px 1fr 360px; grid-template-rows: 1fr auto; }
  #lists { display: block; grid-row: 1 / 3; overflow: auto; border-right: 1px solid var(--line); padding: 8px 0; }
  #browser { display: flex; flex-direction: column; grid-row: 1 / 3; min-width: 0; min-height: 0; }
  #stage { grid-column: 3; border-left: 1px solid var(--line); }
  nav { grid-column: 3; border-left: 1px solid var(--line); }
  nav button { font-size: 1.2rem; padding: 16px 0; }
  #extra { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
  .nextrow { grid-template-columns: 1fr; gap: 6px; }
}
#extra button { background: #2a2a2a; padding: 10px 0; font-size: .95rem; border-radius: 8px; }
#extra #output { grid-column: 1 / 3; }
#extra #output.off { background: var(--danger); }
.keys { grid-column: 1 / 3; font-size: .75rem; color: var(--dim); text-align: center; }

details { border-bottom: 1px solid var(--line); }
summary { padding: 8px 14px; font-weight: 600; cursor: pointer; list-style: none; display: flex; gap: 6px; align-items: center; }
summary::before { content: "▸"; color: var(--dim); }
details[open] > summary::before { content: "▾"; }
.item { display: flex; align-items: center; gap: 8px; width: 100%; text-align: left; background: none; border-radius: 0; padding: 8px 14px 8px 28px; color: #ccc; }
.item:hover { background: #222; }
.item.sel { background: #2b2b2b; color: #fff; }
.item .dot { width: 8px; height: 8px; border-radius: 50%; flex: none; }
.item.live .dot { background: var(--live); }
.item span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.empty { color: var(--dim); padding: 12px 14px; font-size: .9rem; }

.bhead { display: flex; align-items: center; gap: 12px; padding: 10px 16px; border-bottom: 1px solid var(--line); }
#ptitle { font-size: 1.05rem; margin: 0; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
#follow { background: #2a2a2a; padding: 6px 12px; border-radius: 8px; font-size: .85rem; }
#follow[hidden] { display: none; }
#grid { flex: 1; overflow: auto; padding: 12px 16px; display: grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap: 12px; align-content: start; }
#grid .msg { grid-column: 1 / -1; color: var(--dim); }
.tile { background: none; padding: 0; border-radius: 6px; text-align: left; color: #ccc; display: flex; flex-direction: column; overflow: hidden; border: 3px solid transparent; }
.tile .screen { border-radius: 3px; }
.tile.live { border-color: var(--live); }
.tile.nx { border-color: var(--next); }
.tile.off { opacity: .4; }
.tile:hover .screen { outline: 1px solid #555; }
.cap { display: flex; gap: 6px; align-items: center; font-size: .8rem; padding: 4px 2px; }
.cap i { width: 4px; align-self: stretch; border-radius: 2px; background: #555; flex: none; }
.cap span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style></head>
<body>
<header><span class="brand"><svg class="logo" viewBox="0 0 512 512" aria-hidden="true"><defs><linearGradient id="op-bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2A2A31"/><stop offset="1" stop-color="#121215"/></linearGradient><linearGradient id="op-accent" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#FFA845"/><stop offset="1" stop-color="#F06A1D"/></linearGradient></defs><rect width="512" height="512" rx="96" fill="url(#op-bg)"/><g transform="translate(0 6)"><rect x="226" y="320" width="60" height="70" fill="url(#op-accent)"/><rect x="150" y="364" width="212" height="56" rx="28" fill="url(#op-accent)"/><path d="M 280 110 H 180 A 72 72 0 0 0 108 182 V 252 A 72 72 0 0 0 180 324 H 332 A 72 72 0 0 0 404 252 V 246" fill="none" stroke="#FFFFFF" stroke-width="60" stroke-linecap="round"/><rect x="168" y="196" width="124" height="48" rx="24" fill="#FFFFFF"/><path d="M 358 88 L 424 154 L 358 220" fill="none" stroke="url(#op-accent)" stroke-width="56" stroke-linecap="round" stroke-linejoin="round"/></g></svg><span id="title">–</span></span><span id="status" class="off">connecting…</span></header>
<div id="app">
  <aside id="lists" aria-label="Playlists"></aside>
  <section id="browser">
    <div class="bhead"><h2 id="ptitle"></h2><button id="follow" hidden>Follow live</button></div>
    <div id="grid"></div>
  </section>
  <section id="stage">
    <div class="curbox">
      <div class="label"><span>On screen</span><span id="badges"></span></div>
      <div class="screen" id="cur"><img class="bg" alt=""><img class="sl" alt=""><div class="fb"></div></div>
    </div>
    <div class="nextbox">
      <div class="nextrow">
        <div><div class="label">Next</div><div id="nexttitle"></div></div>
        <div class="screen" id="nxt"><img class="bg" alt=""><img class="sl" alt=""><div class="fb"></div></div>
      </div>
    </div>
    <div id="extra">
      <button id="cleartext">Clear Text</button>
      <button id="clearbg">Clear Background</button>
      <button id="output">Output on</button>
      <div class="keys">Keys: → / Space / Page Down next · ← / Page Up previous</div>
    </div>
  </section>
  <nav>
    <button id="prev"><svg class="arrow back" viewBox="0 0 84 134" aria-hidden="true"><path d="M 17 17 L 67 67 L 17 117" fill="none" stroke="currentColor" stroke-width="34" stroke-linecap="round" stroke-linejoin="round"/></svg>Prev</button>
    <button id="nextbtn">Next<svg class="arrow" viewBox="0 0 84 134" aria-hidden="true"><path d="M 17 17 L 67 67 L 17 117" fill="none" stroke="currentColor" stroke-width="34" stroke-linecap="round" stroke-linejoin="round"/></svg></button>
    <div id="holds">
      <button id="freeze">Freeze</button>
      <button id="blank">Blank</button>
    </div>
    <button id="clear">Clear</button>
  </nav>
</div>
<script>
const $ = id => document.getElementById(id);
let S = null;      // live/next state
let SH = null;     // playlists + presentations
let browse = null; // presentation shown in the grid
let follow = true; // grid follows the live presentation
const wide = matchMedia('(min-width: 900px) and (min-height: 500px)');

function setStatus(t, ok) { $('status').textContent = t; $('status').className = ok ? '' : 'off'; }
async function post(path) {
  try {
    const r = await fetch(path, { method: 'POST' });
    if (r.status === 401) location.reload();
  } catch (e) { setStatus('not connected', false); }
}
function cmd(name) { if (navigator.vibrate) navigator.vibrate(10); post('/api/cmd/' + name); }
$('prev').onclick = () => cmd('prev');
$('nextbtn').onclick = () => cmd('next');
$('clear').onclick = () => cmd('clear');
$('freeze').onclick = () => cmd(S && S.frozen ? 'unfreeze' : 'freeze');
$('blank').onclick = () => cmd(S && S.blanked ? 'unblank' : 'blank');
$('cleartext').onclick = () => cmd('clear-text');
$('clearbg').onclick = () => cmd('clear-bg');
$('output').onclick = () => cmd(S && S.output_enabled ? 'output-off' : 'output-on');
$('follow').onclick = () => { follow = true; if (S && S.live) browse = S.live.id; renderBrowser(); };
document.addEventListener('keydown', e => {
  if (e.repeat || e.ctrlKey || e.metaKey || e.altKey) return;
  if (['ArrowRight', 'ArrowDown', 'PageDown', ' '].includes(e.key)) { e.preventDefault(); cmd('next'); }
  else if (['ArrowLeft', 'ArrowUp', 'PageUp'].includes(e.key)) { e.preventDefault(); cmd('prev'); }
});

// Image URL sized for an element (the server rounds widths to steps of 160).
function sized(url, el) {
  const w = Math.min(1920, Math.ceil((el.clientWidth || 400) * (devicePixelRatio || 1) / 160) * 160);
  return url + '?w=' + w;
}
const sleep = ms => new Promise(r => setTimeout(r, ms));
// Fetched images as object URLs (fetch gives the status: 503 = fonts still loading, retry).
const blobs = new Map();
async function load(url) {
  if (blobs.has(url)) return blobs.get(url);
  for (let i = 0; i < 30; i++) {
    let r;
    try { r = await fetch(url); } catch (e) { await sleep(1000); continue; }
    if (r.status === 401) { location.reload(); return null; }
    if (r.status === 503) { await sleep(1000); continue; }
    if (!r.ok) return null;
    const u = URL.createObjectURL(await r.blob());
    blobs.set(url, u);
    if (blobs.size > 60) { const [k, v] = blobs.entries().next().value; blobs.delete(k); URL.revokeObjectURL(v); }
    return u;
  }
  return null;
}
// Show `url` in an <img>, keeping the old picture until the new one is ready.
// No image (or it can't be drawn): the slide text instead.
async function paint(img, fb, url, text, visible) {
  img.dataset.want = url || '';
  fb.textContent = text || '';
  if (!url) { img.classList.remove('on'); fb.classList.toggle('on', !!text && visible); return; }
  const u = await load(url);
  if (img.dataset.want !== url) return;
  if (u) {
    img.onload = () => { const s = img.closest('.screen'); if (s && img.naturalWidth) s.style.aspectRatio = img.naturalWidth + ' / ' + img.naturalHeight; };
    img.src = u;
    img.classList.toggle('on', visible);
    fb.classList.remove('on');
  } else {
    img.classList.remove('on');
    fb.classList.toggle('on', visible);
  }
}
function screen(el, slideUrl, text, bgUrl, textOn, bgOn) {
  const [bg, sl] = el.querySelectorAll('img');
  const fb = el.querySelector('.fb');
  paint(sl, fb, slideUrl ? sized(slideUrl, el) : null, text, textOn);
  if (bgUrl && bgOn) {
    const u = sized(bgUrl, el);
    if (bg.dataset.want !== u) { bg.dataset.want = u; load(u).then(b => { if (bg.dataset.want === u && b) { bg.src = b; bg.classList.add('on'); } }); }
  } else { bg.dataset.want = ''; bg.classList.remove('on'); }
}

function show(s) {
  const prevLive = S && S.live ? S.live.id : null;
  S = s;
  const l = s.live, n = s.next;
  $('title').textContent = l ? `${l.title} · ${l.group} · ${l.position}/${l.total}` : 'Nothing live';
  const cur = $('cur');
  cur.classList.toggle('cleared', !s.text_visible);
  screen(cur, l && l.image, l ? (l.text || '') : '', s.background, s.text_visible, s.background_visible);
  const badges = [];
  if (l && !s.text_visible) badges.push('Text cleared');
  if (!s.output_enabled) badges.push('Output off');
  if (s.blanked) badges.push('Screen blank');
  else if (s.frozen) badges.push('Screen frozen');
  $('badges').replaceChildren(...badges.map(t => { const b = document.createElement('span'); b.className = 'badge'; b.textContent = t; return b; }));
  $('nexttitle').textContent = !n ? (l ? 'End' : '') : (n.same_presentation ? '' : n.title);
  screen($('nxt'), n && n.image, n ? (n.text || '') : '', s.background, true, s.background_visible);
  // Fetch the next slide at the big size now, so Next shows it at once.
  if (n && n.image) load(sized(n.image, cur));
  const o = $('output');
  o.textContent = s.output_enabled ? 'Output on' : 'Output off';
  o.classList.toggle('off', !s.output_enabled);
  const fz = $('freeze'), bk = $('blank');
  fz.textContent = s.frozen ? 'Unfreeze' : 'Freeze';
  fz.classList.toggle('on', s.frozen);
  bk.textContent = s.blanked ? 'Unblank' : 'Blank';
  bk.classList.toggle('on', s.blanked);
  if (follow && l && l.id !== prevLive) browse = l.id;
  if (l && l.id === browse) follow = true; // went live from the grid: follow it again
  renderLists();
  renderBrowser();
}

// Playlist tree: built when the playlists change, marked on every state change.
function buildLists() {
  if (!SH) return;
  const box = $('lists');
  if (!SH.playlists.length) { box.innerHTML = '<div class="empty">No playlists</div>'; return; }
  const open = new Set([...box.querySelectorAll('details[open]')].map(d => d.dataset.i));
  const first = !box.querySelector('details');
  box.replaceChildren(...SH.playlists.map((p, pi) => {
    const d = document.createElement('details');
    d.dataset.i = pi;
    d.open = first || open.has(String(pi));
    const sum = document.createElement('summary');
    sum.textContent = p.name;
    d.append(sum);
    for (const it of p.items) {
      const b = document.createElement('button');
      b.className = 'item';
      b.dataset.id = it.id;
      const dot = document.createElement('i'); dot.className = 'dot';
      const t = document.createElement('span'); t.textContent = it.title;
      b.append(dot, t);
      b.onclick = () => { browse = it.id; follow = !!(S && S.live && S.live.id === it.id); renderLists(); renderBrowser(); };
      d.append(b);
    }
    return d;
  }));
  renderLists();
}
function renderLists() {
  for (const b of $('lists').querySelectorAll('.item')) {
    b.classList.toggle('sel', b.dataset.id === browse);
    b.classList.toggle('live', !!(S && S.live && S.live.id === b.dataset.id));
  }
}

let gridOf = null; // presentation object the grid was built from
let scrolledTo = '';
function renderBrowser() {
  if (!wide.matches) return;
  const grid = $('grid');
  $('follow').hidden = follow || !(S && S.live);
  if (!browse && SH) { const p = SH.playlists.find(p => p.items.length); if (p) { browse = p.items[0].id; renderLists(); } }
  const p = SH && browse ? SH.presentations[browse] : null;
  $('ptitle').textContent = p ? p.title : '';
  if (!p) { gridOf = null; grid.innerHTML = '<div class="msg">Pick something from a playlist.</div>'; return; }
  if (!p.loaded) {
    gridOf = null;
    grid.replaceChildren(Object.assign(document.createElement('div'), { className: 'msg', textContent: p.error || 'Loading…' }));
    if (!p.error) post('/api/load/' + browse);
    return;
  }
  if (gridOf !== p) {
    gridOf = p;
    const id = browse;
    grid.replaceChildren(...p.slides.map((sl, i) => {
      const b = document.createElement('button');
      b.className = 'tile' + (sl.enabled ? '' : ' off');
      b.title = sl.text;
      const sc = document.createElement('div'); sc.className = 'screen';
      const img = document.createElement('img'); img.className = 'sl on'; img.alt = ''; img.loading = 'lazy';
      const fb = document.createElement('div'); fb.className = 'fb'; fb.textContent = sl.text; fb.style.fontSize = '.7rem';
      let tries = 0;
      img.onerror = () => { img.classList.remove('on'); fb.classList.add('on'); if (tries++ < 5) setTimeout(() => { img.src = sized(sl.image, sc) + '&r=' + tries; img.classList.add('on'); fb.classList.remove('on'); }, 1500); };
      sc.append(img, fb);
      const cap = document.createElement('div'); cap.className = 'cap';
      const bar = document.createElement('i'); if (sl.color) bar.style.background = sl.color;
      const t = document.createElement('span'); t.textContent = `${i + 1}. ` + [sl.group, sl.label].filter(Boolean).join(' · ');
      cap.append(bar, t);
      b.append(sc, cap);
      b.onclick = () => post(`/api/goto/${id}/${i + 1}`);
      requestAnimationFrame(() => { img.src = sized(sl.image, sc); });
      return b;
    }));
  }
  const tiles = grid.children;
  let liveTile = null;
  for (let i = 0; i < tiles.length; i++) {
    const isLive = !!(S && S.live && S.live.id === browse && S.live.position === i + 1);
    tiles[i].classList.toggle('live', isLive);
    tiles[i].classList.toggle('nx', !!(S && S.next && S.next.id === browse && S.next.position === i + 1));
    if (isLive) liveTile = tiles[i];
  }
  const at = S && S.live ? S.live.id + ':' + S.live.position : '';
  if (follow && liveTile && at !== scrolledTo) liveTile.scrollIntoView({ block: 'nearest' });
  scrolledTo = at;
}
wide.addEventListener('change', () => { gridOf = null; renderBrowser(); });

function connect() {
  const ws = new WebSocket((location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws');
  ws.onopen = () => setStatus('connected', true);
  ws.onmessage = e => {
    const m = JSON.parse(e.data);
    if (m.playlists) { SH = m; gridOf = null; buildLists(); renderBrowser(); }
    else show(m);
  };
  ws.onclose = e => {
    setStatus('reconnecting…', false);
    if (e.code === 4001) { location.reload(); return; }
    setTimeout(connect, 1000);
  };
}
connect();
</script>
</body></html>"##;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
