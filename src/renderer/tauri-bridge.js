// Riff Tauri 2 Strangler Bridge
// Bridges window.electronAPI calls to Tauri 2 IPC when running under Tauri runtime.
(() => {
  if (window.electronAPI) return; // Running in Electron with native preload

  const isTauri = typeof window.__TAURI_INTERNALS__ !== 'undefined' || typeof window.__TAURI__ !== 'undefined';
  if (!isTauri) return;

  const invoke = window.__TAURI__?.core?.invoke || window.__TAURI_INTERNALS__?.invoke || (async () => {});

  window.electronAPI = {
    isTauri: true,
    isWindows: navigator.userAgent.includes('Windows') || navigator.platform.includes('Win'),
    platform: navigator.userAgent.includes('Windows') ? 'win32' : 'linux',
    serverToken: 'riff-tauri-core',
    minimizeWindow: () => invoke('minimize_window'),
    maximizeWindow: () => invoke('maximize_window'),
    closeWindow: () => invoke('close_window'),
    getServerPort: async () => {
      try {
        const port = await invoke('get_server_port');
        return port || 50406;
      } catch (_) {
        return 50406;
      }
    },
    getAppVersion: async () => {
      try { return await invoke('get_app_version'); } catch (_) { return '0.7.6'; }
    },
    openExternal: (url) => invoke('open_external', { url: String(url) }),
    getDataDir: () => invoke('get_data_dir'),
    onWindowVisibility: () => {},
    discordRpcUpdate: () => {},
    discordRpcClear: () => {},
    discordRpcSetEnabled: () => {},
    discordRpcGetEnabled: async () => false,
    discordRpcGetConnected: async () => false,
    discordRpcReconnect: () => {},
    getCacheSize: async () => ({ size: 0 }),
    clearCache: async () => ({ success: true }),
    logError: (level, message, stack) => console.error(`[${level}]`, message, stack),
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
    storageInitSync: () => ({}),
    storageSaveBatch: () => {},
    storageSaveSync: () => {}
  };
})();
