//! The two pages served to phones, embedded in the program (nothing to deploy).

pub fn login(message: Option<&str>) -> String {
    let msg = message
        .map(|m| format!("<p class=\"err\">{}</p>", escape(m)))
        .unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1">
<title>PP6 Remote</title>
<style>
body {{ font-family: system-ui, sans-serif; background: #111; color: #eee; margin: 0; display: flex; min-height: 100vh; align-items: center; justify-content: center; }}
form {{ background: #1d1d1d; padding: 24px; border-radius: 12px; width: min(90vw, 320px); }}
h1 {{ font-size: 1.3rem; margin: 0 0 16px; }}
input {{ width: 100%; box-sizing: border-box; font-size: 2rem; letter-spacing: .4em; text-align: center; padding: 10px; border-radius: 8px; border: 1px solid #444; background: #000; color: #fff; }}
button {{ width: 100%; margin-top: 16px; font-size: 1.2rem; padding: 14px; border: 0; border-radius: 8px; background: #2a6df4; color: #fff; }}
.err {{ color: #ff7b7b; }}
p {{ color: #aaa; font-size: .9rem; }}
</style></head>
<body><form method="post" action="/login">
<h1>PP6 Remote</h1>
<p>Enter the PIN shown on the operator screen.</p>
{msg}
<input name="pin" inputmode="numeric" pattern="[0-9]*" autocomplete="one-time-code" maxlength="6" autofocus required>
<button type="submit">Connect</button>
</form></body></html>"#
    )
}

pub const REMOTE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no">
<title>PP6 Remote</title>
<style>
* { box-sizing: border-box; -webkit-tap-highlight-color: transparent; }
body { font-family: system-ui, sans-serif; background: #111; color: #eee; margin: 0; display: flex; flex-direction: column; height: 100vh; height: 100dvh; touch-action: manipulation; }
header { padding: 8px 12px; font-size: .9rem; color: #aaa; display: flex; justify-content: space-between; gap: 8px; }
#status.off { color: #ff7b7b; }
main { flex: 1; padding: 0 12px; overflow: auto; }
.label { font-size: .75rem; text-transform: uppercase; color: #888; margin-top: 10px; }
#current { font-size: 1.5rem; white-space: pre-line; background: #000; border: 2px solid #f39a2b; border-radius: 8px; padding: 12px; min-height: 5em; }
#current.cleared { opacity: .35; }
#next { font-size: 1.05rem; white-space: pre-line; color: #bbb; background: #1a1a1a; border-radius: 8px; padding: 10px; min-height: 3em; }
nav { display: grid; grid-template-columns: 1fr 2fr; grid-template-rows: auto auto; gap: 10px; padding: 12px; }
button { font-size: 1.4rem; padding: 22px 0; border: 0; border-radius: 12px; color: #fff; }
#prev { background: #333; }
#nextbtn { background: #2a6df4; }
#clear { grid-column: 1 / 3; background: #7a2323; padding: 14px 0; font-size: 1.1rem; }
button:active { filter: brightness(1.3); }
</style></head>
<body>
<header><span id="title">–</span><span id="status" class="off">connecting…</span></header>
<main>
  <div class="label" id="curlabel">On screen</div>
  <div id="current"></div>
  <div class="label" id="nextlabel">Next</div>
  <div id="next"></div>
</main>
<nav>
  <button id="prev">◀ Prev</button>
  <button id="nextbtn">Next ▶</button>
  <button id="clear">Clear</button>
</nav>
<script>
const $ = id => document.getElementById(id);
async function cmd(name) {
  try {
    const r = await fetch('/api/cmd/' + name, { method: 'POST' });
    if (r.status === 401) location.reload();
  } catch (e) { $('status').textContent = 'not connected'; $('status').className = 'off'; }
}
$('prev').onclick = () => cmd('prev');
$('nextbtn').onclick = () => cmd('next');
$('clear').onclick = () => cmd('clear');
function show(s) {
  const l = s.live;
  $('title').textContent = l ? `${l.title} · ${l.group} · ${l.position}/${l.total}` : 'Nothing live';
  $('current').textContent = l ? (l.text || '(no text)') : '';
  $('current').className = s.text_visible ? '' : 'cleared';
  $('curlabel').textContent = s.text_visible ? 'On screen' : 'On screen (cleared)';
  const n = s.next;
  $('nextlabel').textContent = n && !n.same_presentation ? `Next: ${n.title}` : 'Next';
  $('next').textContent = n ? (n.text || '(no text)') : '';
}
function connect() {
  const ws = new WebSocket((location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws');
  ws.onopen = () => { $('status').textContent = 'connected'; $('status').className = ''; };
  ws.onmessage = e => show(JSON.parse(e.data));
  ws.onclose = e => {
    $('status').textContent = 'reconnecting…'; $('status').className = 'off';
    if (e.code === 4001) { location.reload(); return; }
    setTimeout(connect, 1000);
  };
}
connect();
</script>
</body></html>"#;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
