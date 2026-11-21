const { spawn } = require('child_process');
const path = require('path');
const fs = require('fs');
const http = require('http');

const exe = path.resolve('node_modules/electron/dist/electron.exe');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function getJson(url, token) {
  return new Promise((resolve, reject) => {
    const u = new URL(url);
    const req = http.request({
      hostname: u.hostname,
      port: u.port,
      path: u.pathname + u.search,
      headers: token ? { 'x-riff-token': token } : {}
    }, (res) => {
      let data = '';
      res.on('data', (c) => (data += c));
      res.on('end', () => {
        try { resolve(JSON.parse(data)); } catch (e) { resolve(data); }
      });
    });
    req.on('error', reject);
    req.end();
  });
}

async function testBackend(type) {
  const tmpDir = path.join(process.env.TEMP, `riff-bench-${type}`);
  fs.rmSync(tmpDir, { recursive: true, force: true });
  fs.mkdirSync(tmpDir, { recursive: true });

  const t0 = Date.now();
  const sm = require('../src/main/server-manager');
  process.env.RIFF_BACKEND = type;
  process.env.RIFF_DATA_DIR = tmpDir;

  const port = await sm.startServer(0);
  const startupMs = Date.now() - t0;
  const token = sm.getServerToken();

  const lib = await getJson(`http://127.0.0.1:${port}/api/library`, token);

  sm.stopServer();
  await sleep(300);
  try { fs.rmSync(tmpDir, { recursive: true, force: true }); } catch (_) {}

  return {
    backend: type,
    startupMs,
    port,
    libraryResult: Array.isArray(lib) ? `array(${lib.length})` : typeof lib
  };
}

(async () => {
  console.log('Testing backends...');
  const nodeRes = await testBackend('node');
  console.log('Node result:', nodeRes);

  const rustRes = await testBackend('rust');
  console.log('Rust result:', rustRes);
  process.exit(0);
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
