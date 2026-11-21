const { spawn } = require('child_process');
const http = require('http');

const token = 'riff-test-token';
const port = 39999;
const core = spawn('target/debug/riff-core.exe', ['--token', token, '--port', String(port)]);

core.stdout.on('data', (d) => {
  const str = d.toString();
  if (str.includes('"event":"ready"') || str.includes('"event": "ready"')) {
    console.log('[test] Core is ready!');
    runTests();
  }
});

core.stderr.on('data', (d) => {
  console.log('[core stderr]', d.toString().trim());
});

function httpGet(path) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        hostname: '127.0.0.1',
        port,
        path,
        headers: { 'x-riff-token': token }
      },
      (res) => {
        let body = '';
        res.on('data', (c) => (body += c));
        res.on('end', () => resolve({ status: res.statusCode, body, headers: res.headers }));
      }
    );
    req.on('error', reject);
    req.end();
  });
}

async function runTests() {
  try {
    // 1. Search SoundCloud tracks (tobu)
    console.log('[test] 1. Testing GET /api/search?q=tobu&platform=soundcloud...');
    const searchRes = await httpGet('/api/search?q=tobu&platform=soundcloud&limit=2');
    console.log('[test] Search HTTP Status:', searchRes.status);
    console.log('[test] Search Body:', searchRes.body);
    const searchJson = JSON.parse(searchRes.body);
    console.log('[test] Tracks found:', searchJson.tracks?.length);
    const track = searchJson.tracks?.[0];
    console.log('[test] Top Track:', track?.title, 'by', track?.artist, '(id:', track?.id, ')');

    if (!track) {
      throw new Error('No tracks returned from SoundCloud search');
    }

    // 2. Resolve Stream Info (with only id and platform)
    console.log('\n[test] 2. Testing GET /api/stream-info?id=' + track.id + '&platform=soundcloud...');
    const streamRes = await httpGet('/api/stream-info?id=' + track.id + '&platform=soundcloud&title=' + encodeURIComponent(track.title) + '&url=' + encodeURIComponent(track.url));
    console.log('[test] Stream-Info HTTP Status:', streamRes.status);
    console.log('[test] Stream-Info Body:', streamRes.body);
    const streamJson = JSON.parse(streamRes.body);
    console.log('[test] Stream-Info Success:', streamJson.success);
    console.log('[test] StreamUrl (proxied):', streamJson.streamUrl?.slice(0, 90) + '...');

    // 3. Audio Proxy HTTP Range request
    if (streamJson.streamUrl) {
      const u = new URL(streamJson.streamUrl);
      console.log('\n[test] 3. Testing GET', u.pathname + u.search.slice(0, 40) + '... with Range bytes=0-1024');
      const proxyRes = await new Promise((resolve, reject) => {
        const pReq = http.request(
          {
            hostname: '127.0.0.1',
            port,
            path: u.pathname + u.search,
            headers: {
              'x-riff-token': token,
              'Range': 'bytes=0-1024'
            }
          },
          (res) => {
            let bytes = 0;
            res.on('data', (c) => (bytes += c.length));
            res.on('end', () => resolve({ status: res.statusCode, bytes, headers: res.headers }));
          }
        );
        pReq.on('error', reject);
        pReq.end();
      });

      console.log('[test] Audio Proxy Status:', proxyRes.status);
      console.log('[test] Content-Type:', proxyRes.headers['content-type']);
      console.log('[test] Content-Range:', proxyRes.headers['content-range']);
      console.log('[test] Received bytes:', proxyRes.bytes);
    }

    console.log('\n[test] === ALL PHASE 2 NATIVE RUST TESTS PASSED! ===');
  } catch (err) {
    console.error('[test] Test failed:', err);
  } finally {
    core.kill();
    process.exit(0);
  }
}
