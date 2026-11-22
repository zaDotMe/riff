const http = require('http');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

function request(url, headers = {}) {
  return new Promise((resolve, reject) => {
    const t0 = Date.now();
    const u = new URL(url);
    const req = http.request({
      hostname: u.hostname,
      port: u.port,
      path: u.pathname + u.search,
      headers
    }, (res) => {
      let data = '';
      res.on('data', c => (data += c));
      res.on('end', () => {
        const ms = Date.now() - t0;
        try {
          resolve({ status: res.statusCode, data: JSON.parse(data), ms });
        } catch (_) {
          resolve({ status: res.statusCode, data, ms });
        }
      });
    });
    req.on('error', reject);
    req.end();
  });
}

function getMemoryUsage() {
  try {
    const psScript = `
      $ProgressPreference = 'SilentlyContinue';
      $r = Get-Process riff-tauri -ErrorAction SilentlyContinue | Select-Object -First 1;
      $allW = Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'";
      $w = $allW | Where-Object { $_.CommandLine -match 'com\\.rootscripts\\.riff' -or ($r -and $_.ParentProcessId -eq $r.Id) };
      $rWs = if ($r) { [math]::Round($r.WorkingSet64 / 1MB, 2) } else { 0 };
      $rPr = if ($r) { [math]::Round($r.PrivateMemorySize64 / 1MB, 2) } else { 0 };
      $wWs = if ($w) { [math]::Round(($w | Measure-Object WorkingSetSize -Sum).Sum / 1MB, 2) } else { 0 };
      $wPr = if ($w) { [math]::Round(($w | Measure-Object PageFileUsage -Sum).Sum / 1MB, 2) } else { 0 };
      $wCount = if ($w) { $w.Count } else { 0 };
      [PSCustomObject]@{
        RiffPid = if ($r) { $r.Id } else { 0 };
        RiffWsMb = $rWs;
        RiffPrivateMb = $rPr;
        WebViewCount = $wCount;
        WebViewWsMb = $wWs;
        WebViewPrivateMb = $wPr;
        TotalWsMb = [math]::Round($rWs + $wWs, 2);
        TotalPrivateMb = [math]::Round($rPr + $wPr, 2);
      } | ConvertTo-Json -Compress
    `;
    const b64 = Buffer.from(psScript, 'utf16le').toString('base64');
    const out = execSync(`powershell -NoProfile -EncodedCommand ${b64}`, { encoding: 'utf8' }).trim();
    return JSON.parse(out);
  } catch (e) {
    return { error: e.message };
  }
}

async function runBenchmark() {
  console.log('='.repeat(70));
  console.log('  RIFF LEAN STACK (TAURI 2 SINGLE-BINARY) BENCHMARK');
  console.log('='.repeat(70));

  const exePath = path.resolve(__dirname, '../target/release/riff-tauri.exe');
  let exeSizeMb = 0;
  if (fs.existsSync(exePath)) {
    exeSizeMb = (fs.statSync(exePath).size / (1024 * 1024)).toFixed(2);
    console.log(`\n[1] Standalone Binary Size: ${exeSizeMb} MB (${exePath})`);
  } else {
    console.log(`\n[1] Binary not found at: ${exePath}`);
  }

  const port = 38472;
  const token = 'riff-tauri-core';
  console.log(`\n[2] Testing Backend Endpoints (http://127.0.0.1:${port}):`);

  // Health
  try {
    const health = await request(`http://127.0.0.1:${port}/api/health`);
    console.log(`  - /api/health: HTTP ${health.status} in ${health.ms}ms (status: ${health.data?.status || 'ok'})`);
  } catch (e) {
    console.log(`  - /api/health: FAIL (${e.message})`);
  }

  // Library
  try {
    const lib = await request(`http://127.0.0.1:${port}/api/library`, { 'x-riff-token': token });
    console.log(`  - /api/library (SQLite WAL): HTTP ${lib.status} in ${lib.ms}ms (tracks: ${Array.isArray(lib.data) ? lib.data.length : 'ok'})`);
  } catch (e) {
    console.log(`  - /api/library: FAIL (${e.message})`);
  }

  // Search YTMusic
  try {
    const sYtm = await request(`http://127.0.0.1:${port}/api/search?q=sheila+on+7+dan&platform=ytmusic&limit=5`);
    const count = sYtm.data?.tracks?.length || 0;
    const first = sYtm.data?.tracks?.[0]?.title || '-';
    console.log(`  - /api/search (YT Music WEB_REMIX): HTTP ${sYtm.status} in ${sYtm.ms}ms (found: ${count}, top: "${first}")`);
  } catch (e) {
    console.log(`  - /api/search (YT Music): FAIL (${e.message})`);
  }

  // Search SoundCloud
  try {
    const sSc = await request(`http://127.0.0.1:${port}/api/search?q=coldplay+yellow&platform=soundcloud&limit=5`);
    const count = sSc.data?.tracks?.length || 0;
    const first = sSc.data?.tracks?.[0]?.title || '-';
    console.log(`  - /api/search (SoundCloud native): HTTP ${sSc.status} in ${sSc.ms}ms (found: ${count}, top: "${first}")`);
  } catch (e) {
    console.log(`  - /api/search (SoundCloud): FAIL (${e.message})`);
  }

  // Stream Info Smart Mirror
  try {
    const sInfo = await request(`http://127.0.0.1:${port}/api/stream-info?id=k1BfsO0mxWQ&platform=ytmusic&title=Dan...&artist=Sheila+On+7&duration=289&quality=medium`);
    const isHls = sInfo.data?.isHls;
    const host = sInfo.data?.rawStreamUrl ? new URL(sInfo.data.rawStreamUrl).host : '-';
    console.log(`  - /api/stream-info (Smart Mirror HLS): HTTP ${sInfo.status} in ${sInfo.ms}ms (isHls: ${isHls}, host: ${host})`);
  } catch (e) {
    console.log(`  - /api/stream-info (Smart Mirror): FAIL (${e.message})`);
  }

  // Tools status
  try {
    const tools = await request(`http://127.0.0.1:${port}/api/tools-status`, { 'x-riff-token': token });
    console.log(`  - /api/tools-status: HTTP ${tools.status} in ${tools.ms}ms (ytdlp: ${tools.data?.ytdlp?.found}, ffmpeg: ${tools.data?.ffmpeg?.found})`);
  } catch (e) {
    console.log(`  - /api/tools-status: FAIL (${e.message})`);
  }

  console.log('\n[3] Memory Profiling (Real Working Set & Private Bytes):');
  const mem = getMemoryUsage();
  if (mem.error) {
    console.log('  Failed to get memory metrics:', mem.error);
  } else {
    console.log(`  - riff-tauri.exe (PID ${mem.RiffPid}, In-process Axum/Tokio/Core):`);
    console.log(`      * Private Memory (Task Manager default): ${mem.RiffPrivateMb} MB`);
    console.log(`      * Working Set (Physical RAM):             ${mem.RiffWsMb} MB`);
    console.log(`  - Edge WebView2 (${mem.WebViewCount} processes: Host, GPU, Audio, Renderer):`);
    console.log(`      * Private Memory:                         ${mem.WebViewPrivateMb} MB`);
    console.log(`      * Working Set:                            ${mem.WebViewWsMb} MB`);
    console.log(`  - Total Combined Riff Footprint:`);
    console.log(`      * Total Private Memory:                   ${mem.TotalPrivateMb} MB`);
    console.log(`      * Total Physical RAM (Working Set):       ${mem.TotalWsMb} MB`);
  }

  console.log('\n' + '='.repeat(70));
  console.log('  BENCHMARK COMPLETE');
  console.log('='.repeat(70) + '\n');
}

runBenchmark().catch(console.error);
