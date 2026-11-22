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
    pickDataDir: async () => {
      try { return await getInvoke()('pick_data_dir'); } catch (_) { return null; }
    },
    setDataDir: async () => true,
    getDownloadDir: async () => {
      try { return await getInvoke()('get_download_dir'); } catch (_) { return ''; }
    },
    pickDownloadDir: async () => {
      try { return await getInvoke()('pick_download_dir'); } catch (_) { return null; }
    },
    setDownloadDir: async () => true,
    resetDownloadDir: async () => true,
    pickMedia: async () => {
      try {
        const filePath = await getInvoke()('pick_media');
        if (!filePath) return null;
        if (window.__TAURI__?.core?.convertFileSrc) {
          return window.__TAURI__.core.convertFileSrc(filePath);
        }
        return 'file:///' + filePath.replace(/\\/g, '/');
      } catch (_) {
        return null;
      }
    },
    pickLocalFiles: async () => {
      try {
        return (await getInvoke()('pick_local_files')) || [];
      } catch (e) {
        console.warn('pickLocalFiles error:', e);
        return [];
      }
    },
    pickLocalFolder: async () => {
      try {
        return (await getInvoke()('pick_local_folder')) || [];
      } catch (e) {
        console.warn('pickLocalFolder error:', e);
        return [];
      }
    },
    importAudioFiles: async () => {
      try {
        return (await getInvoke()('import_audio_files')) || [];
      } catch (e) {
        console.warn('importAudioFiles error:', e);
        return [];
      }
    },
    rescanLocalMetadata: async () => [],
    readCacheFile: async () => null,
    writeCacheFile: () => {},
    getGpuCompositing: async () => true,
    setAppProxy: async () => true,
    restartApp: () => {
      try { getInvoke()('restart_app'); } catch (_) {}
    },
    googleLogin: async () => ({ success: false }),
    googleLogout: async () => ({ success: true }),
    googleAccountInfo: async () => null,
    googleImportLibrary: async () => ({ success: false }),
    googleAuthStatus: async () => ({ loggedIn: false }),
    youtubePlaybackLogin: async () => false,
    youtubePlaybackLogout: async () => true,
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
    getAudioOutputInfo: async () => ({ device: 'default' }),
    checkForUpdates: async () => ({ available: false }),
    updateAction: () => {},
    installUpdateNow: () => {},
    onUpdateAvailable: () => {},
    onUpdateDownloaded: () => {},
    onUpdateProgress: () => {},
    onUpdateReady: () => {},
    onUpdateError: () => {},
    onUpdateNotAvailable: () => {},
    savePlaylistsJson: async () => true,
    showPlaylistContextMenu: async () => {},
    accountExport: async () => null,
    accountImport: async () => null,
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
    openLogsFolder: () => {
      try { getInvoke()('open_logs_folder'); } catch (_) {}
    },
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

  // Enable native titlebar dragging for Tauri 2 WebView2
  window.addEventListener('mousedown', (e) => {
    if (e.buttons === 1) {
      const titlebar = e.target.closest('.titlebar-drag, .titlebar');
      const isInteractive = e.target.closest('button, input, select, a, .win-btn, .app-logo, .no-drag, [data-no-drag]');
      if (titlebar && !isInteractive) {
        try { getInvoke()('start_drag'); } catch (_) {}
      }
    }
  });
})();
