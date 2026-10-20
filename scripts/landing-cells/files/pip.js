// Puts the YouTube video playing in Edge into Edge's picture-in-picture window,
// as a click on its button would: through Edge's debugging port, with a user's gesture.
//   node --experimental-websocket pip.js <port>
const port = process.argv[2] || 9230;
const sleep = ms => new Promise(r => setTimeout(r, ms));
(async () => {
  let page;
  for (let i = 0; i < 40 && !page; i++) {
    try { page = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find(t => t.type === 'page' && t.url.includes('youtube.com/watch')); } catch {}
    if (!page) await sleep(500);
  }
  if (!page) throw new Error('no video page');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((ok, no) => { ws.onopen = ok; ws.onerror = no; });
  let id = 0; const waiting = {};
  ws.onmessage = m => { const d = JSON.parse(m.data); if (waiting[d.id]) { waiting[d.id](d); delete waiting[d.id]; } };
  const run = expression => new Promise(ok => {
    waiting[++id] = ok;
    ws.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, userGesture: true, awaitPromise: true, returnByValue: true } }));
  }).then(d => d.result && d.result.result ? d.result.result.value : d);
  // The video itself playing, not an advert before it.
  let ready = false;
  for (let i = 0; i < 60 && !ready; i++) {
    ready = await run(`(() => { const v = document.querySelector('video'); return !!v && v.readyState > 2 && !v.paused && !document.querySelector('.ad-showing'); })()`);
    if (!ready) await sleep(500);
  }
  console.log(JSON.stringify({ ready, pip: await run(`document.querySelector('video').requestPictureInPicture().then(() => 'ok', e => String(e))`) }));
  ws.close();
})().catch(e => { console.log(String(e)); process.exit(1); });
