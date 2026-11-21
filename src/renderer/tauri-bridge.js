// Riff Tauri 2 Strangler Bridge
// Bridges window.electronAPI calls to Tauri 2 IPC when running under Tauri runtime.
(() => {
  if (window.electronAPI && !window.electronAPI.isTauri) return; // Running in Electron with native preload

  const getInvoke = () => {
    return (
      window.__TAURI__?.core?.invoke ||
      window.__TAURI_INTERNALS__?.invoke ||
      window.__TAURI__?.invoke ||
      (async () => {})
    );
  };

  window.electronAPI = {
    isTauri: true,
    isWindows: navigator.userAgent.includes('Windows') || navigator.platform.includes('Win'),
    platform: navigator.userAgent.includes('Windows') ? 'win32' : 'linux',
    serverToken: 'riff-tauri-core',
    minimizeWindow: () => getInvoke()('minimize_window'),
    maximizeWindow: () => getInvoke()('maximize_window'),
    closeWindow: () => getInvoke()('close_window'),
    getServerPort: async () => {
      try {
        const port = await getInvoke()('get_server_port');
        return port || 38472;
      } catch (_) {
        return 38472;
      }
    },
    getAppVersion: async () => {
      try { return await getInvoke()('get_app_version'); } catch (_) { return '0.7.6'; }
    },
    openExternal: (url) => getInvoke()('open_external', { url: String(url) }),
    getDataDir: () => getInvoke()('get_data_dir'),
    readCacheFile: async () => null,
    writeCacheFile: () => {},
    getGpuCompositing: async () => true,
    setAppProxy: async () => true,
    googleAuthStatus: async () => ({ loggedIn: false }),
    youtubePlaybackStatus: async () => ({ loggedIn: false }),
    onYoutubePlaybackAuth: () => {},
    onWindowVisibility: () => {},
    discordRpcUpdate: () => {},
    discordRpcClear: () => {},
    discordRpcSetEnabled: () => {},
    discordRpcGetEnabled: async () => false,
    discordRpcGetConnected: async () => false,
    discordRpcReconnect: () => {},
    getCacheSize: async () => ({ size: 0 }),
    clearCache: async () => ({ success: true }),
    logError: (level, message, stack) => {
      console.error(`[${level}]`, message, stack);
      try {
        getInvoke()('log_error', {
          level: String(level),
          message: String(message),
          stack: stack ? String(stack) : null,
          filename: null,
          line: null
        });
      } catch (_) {}
    },
    openLogsFolder: () => {},
    getDiagnostics: async () => `Riff Tauri 2 Runtime\nOS: ${navigator.platform}`,
    setZoomFactor: () => {},
    getZoomFactor: () => 1.0,
    clearWebFrameCache: () => {},
    setTrayOnClose: () => {},
    getMultiInstance: async () => false,
    setMultiInstance: () => {},
    getGlobalShortcuts: async () => ({}),
    setGlobalShortcuts: () => {},
    onMediaCommand: () => {},
    onMediaControl: () => {},
    toggleMiniWindow: () => {},
    setMiniLyrics: () => {},
    setWindowButtonsVisible: () => {},
    setMediaKeys: () => {},
    mediaKeysActive: () => {},
    welcomeFinish: () => {},
    onWelcomeResult: () => {},
    listThemes: async () => [],
    importThemes: async () => [],
    setThemeEnabled: () => {},
    disableAllThemes: () => {},
    removeTheme: () => {},
    listPlugins: async () => [],
    setPluginEnabled: () => {},
    disableAllPlugins: () => {},
    openPluginsFolder: () => {},
    removePlugin: () => {},
    importPlugins: async () => [],
    getAppCss: async () => '',
    storageInitSync: () => {
      // In web localStorage or fallback
      try {
        const stored = {};
        for (let i = 0; i < localStorage.length; i++) {
          const k = localStorage.key(i);
          if (k) stored[k] = localStorage.getItem(k);
        }
        return stored;
      } catch (_) {
        return {};
      }
    },
    storageSaveBatch: (data) => {
      try {
        if (data && typeof data === 'object') {
          for (const [k, v] of Object.entries(data)) {
            localStorage.setItem(k, String(v));
          }
        }
      } catch (_) {}
      try { getInvoke()('save_storage_batch', { data }); } catch (_) {}
    },
    storageSaveSync: (data) => {
      try {
        if (data && typeof data === 'object') {
          for (const [k, v] of Object.entries(data)) {
            localStorage.setItem(k, String(v));
          }
        }
      } catch (_) {}
      try { getInvoke()('save_storage_batch', { data }); } catch (_) {}
      return true;
    }
  };
})();
