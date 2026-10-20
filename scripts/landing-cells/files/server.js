// Serves the demo pages the browser windows show while the cells are captured.
const http = require('http'), fs = require('fs'), path = require('path');
const root = path.join(__dirname, 'site');
http.createServer((req, res) => {
  let p = decodeURIComponent(req.url.split('?')[0]);
  if (p.endsWith('/')) p += 'index.html';
  if (!path.extname(p)) p += '.html';
  fs.readFile(path.join(root, path.normalize(p)), (err, data) => {
    if (err) { res.writeHead(404); res.end(); return; }
    res.writeHead(200, { 'content-type': p.endsWith('.svg') ? 'image/svg+xml' : 'text/html; charset=utf-8' });
    res.end(data);
  });
}).listen(5173, '127.0.0.1');
