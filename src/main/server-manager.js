const path = require('path');
const fs = require('fs');
const { spawn } = require('child_process');
const readline = require('readline');
const nodeServer = require('../server/server');

let rustProcess = null;
let activePort = nodeServer.PORT;
let activeToken = nodeServer.getServerToken();
let activeBackend = 'node';

function resolveRustBinary() {
  if (process.env.RIFF_CORE_BIN && fs.existsSync(process.env.RIFF_CORE_BIN)) {
    return path.resolve(process.env.RIFF_CORE_BIN);
  }

  const root = path.resolve(__dirname, '../..');
  const candidates = [
    path.join(root, 'target', 'release', process.platform === 'win32' ? 'riff-core.exe' : 'riff-core'),
    path.join(root, 'target', 'debug', process.platform === 'win32' ? 'riff-core.exe' : 'riff-core'),
    path.join(root, 'bin', process.platform === 'win32' ? 'riff-core.exe' : 'riff-core')
  ];

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) return candidate;
  }
  return null;
}

function shouldPreferRust() {
  const backend = (process.env.RIFF_BACKEND || '').toLowerCase();
  if (backend === 'node') return false;
  if (backend === 'rust') return true;
  if (process.env.RIFF_USE_RUST === '0' || process.env.RIFF_USE_RUST === 'false') return false;
  return Boolean(resolveRustBinary());
}

function startRustServer(binPath, requestedPort) {
  return new Promise((resolve, reject) => {
    let resolved = false;
    const args = ['--port', String(requestedPort || 0)];

    if (process.env.RIFF_DATA_DIR) {
      args.push('--data-dir', process.env.RIFF_DATA_DIR);
    }
    if (process.env.RIFF_LIBRARY_DIR) {
      args.push('--library-dir', process.env.RIFF_LIBRARY_DIR);
    }

    try {
      rustProcess = spawn(binPath, args, {
        stdio: ['ignore', 'pipe', 'pipe'],
        windowsHide: true,
        env: { ...process.env }
      });
    } catch (err) {
      return reject(err);
    }

    const rl = readline.createInterface({ input: rustProcess.stdout });
    const timer = setTimeout(() => {
      if (!resolved) {
        resolved = true;
        try { rustProcess.kill(); } catch (_) {}
        reject(new Error('riff-core startup timed out after 5000ms'));
      }
    }, 5000);

    rl.on('line', (line) => {
      try {
        const data = JSON.parse(line);
        if (data && data.event === 'ready' && data.port) {
          clearTimeout(timer);
          resolved = true;
          activePort = data.port;
          activeToken = data.token || activeToken;
          activeBackend = 'rust';
          console.log(`riff-core (rust) running on http://127.0.0.1:${activePort} (pid: ${data.pid})`);
          resolve(activePort);
        }
      } catch (_) {}
    });

    rustProcess.stderr.on('data', (chunk) => {
      const msg = chunk.toString();
      if (process.env.RIFF_DEBUG) {
        console.error(`[riff-core] ${msg}`);
      }
    });

    rustProcess.on('error', (err) => {
      if (!resolved) {
        clearTimeout(timer);
        resolved = true;
        reject(err);
      }
    });

    rustProcess.on('exit', (code, sig) => {
      if (!resolved) {
        clearTimeout(timer);
        resolved = true;
        reject(new Error(`riff-core exited before ready (code: ${code}, signal: ${sig})`));
      } else {
        console.warn(`riff-core exited with code ${code}`);
      }
      rustProcess = null;
    });

    const cleanup = () => {
      if (rustProcess && !rustProcess.killed) {
        try { rustProcess.kill(); } catch (_) {}
      }
    };
    process.once('exit', cleanup);
    process.once('SIGINT', cleanup);
    process.once('SIGTERM', cleanup);
  });
}

async function startServer(port = nodeServer.PORT) {
  if (shouldPreferRust()) {
    const bin = resolveRustBinary();
    if (bin) {
      try {
        console.log(`Starting Rust core backend from ${bin}...`);
        const p = await startRustServer(bin, port);
        return p;
      } catch (err) {
        console.warn(`Failed to start riff-core (${err.message}). Falling back to Node server.`);
      }
    } else {
      console.warn('RIFF_BACKEND=rust was requested but no riff-core binary was found. Falling back to Node server.');
    }
  }

  activeBackend = 'node';
  const p = await nodeServer.startServer(port);
  activePort = p;
  activeToken = nodeServer.getServerToken();
  return p;
}

function stopServer() {
  if (rustProcess && !rustProcess.killed) {
    try { rustProcess.kill(); } catch (_) {}
    rustProcess = null;
  }
  return nodeServer.stopServer();
}

function getServerToken() {
  return activeToken;
}

function getBackendType() {
  return activeBackend;
}

module.exports = {
  startServer,
  stopServer,
  getServerToken,
  getBackendType,
  PORT: nodeServer.PORT,
  resolveRustBinary
};
