// tauri-agent-tools replacement: run JS in the dev app's webview via the
// dev bridge. Auto-discovers the newest /tmp/tauri-dev-bridge-*.token.
// Usage: node scripts/agent-eval.mjs '<js expression>'
import fs from 'node:fs';

const js = process.argv[2];
if (!js) {
  console.error('usage: agent-eval.mjs <js>');
  process.exit(2);
}
const files = fs
  .readdirSync('/tmp')
  .filter((n) => n.startsWith('tauri-dev-bridge-') && n.endsWith('.token'))
  .map((n) => ({ n, mtime: fs.statSync('/tmp/' + n).mtimeMs }))
  .sort((a, b) => b.mtime - a.mtime);
if (!files.length) {
  console.error('no bridge token found (is the dev app running?)');
  process.exit(1);
}
const { port, token } = JSON.parse(fs.readFileSync('/tmp/' + files[0].n, 'utf8'));
const res = await fetch(`http://127.0.0.1:${port}/eval`, {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({ js, token })
});
const body = await res.json().catch(() => ({}));
if (!res.ok) {
  console.error(`bridge ${res.status}:`, JSON.stringify(body).slice(0, 300));
  process.exit(1);
}
console.log(typeof body.result === 'string' ? body.result : JSON.stringify(body.result));
