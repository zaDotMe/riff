# Migration Inventory — Legacy WebView2 UI → Rust + Slint (riff)

> Dokumen hasil audit Fase 0 (pr constants: id, klass, timeline, easing, durasi, trigger, retry). — updated for current codebase (2026-11-22).
>
> **Konteks aplikasi**
> - Stack lama: HTML/CSS/JS (ES6+, jQuery-free, ES modules) + **WebView2** (Tauri 2) + Axum backend in-process (riff-core).
> - Stack baru: **Rust** + **Slint** native → `target/release/riff-slint.exe` (dual launcher di `src-tauri/src/main.rs`, `ui_mode` = "web" | "native" | "slint").
> - Platform target: **Windows** (target utama, app sudah build), Slint cross-platform dimungkinkan.
> - App: desktop music player (audio + lyrics + library + social import).
> - Sumber UI (lama): `src/renderer` (index.html 66k+ LOC, styles.css ~20k LOC, ~15 modules).
> - Backend: Axum Axum router `riff-core` (main.rs, db.rs WAL SQLite, library.rs Lofty, stream.rs yt-dlp) via `window.electronAPI.*` (bridge di `src/renderer/tauri-bridge.js`).
> - State: global JS `state.js` + per-feature modules (ui.js, library.js, audio.js, lyrics.js, audioeditor.js, theme.js); storage via `window.__RIFF_STORAGE_MAP__` → `electronAPI.storageSaveSync/Batch`, cache file (`readCacheFile/writeCacheFile`), localStorage.
> - Locale: id-ID utama (data-i18n tags; welcome.html = setup dalam Bahasa Inggris).
> - Accessibility: keyboard-nav primitif (m3 dropdowns role=button+tabindex, tabindex 0 pada artist section headers), aria-labels di banyak button, kurang di track rows/toast/canvas.
> - OS/hardware: Windows 10+, layar 1280×720+, touch opsional (mini lyrics).
> - Design source of truth: `src/renderer/styles.css` + `index.html` (CSS+HTML is source of truth; warna/spasi dasar dari `:root` di styles.css) + reference screenshot dokumen-dokumen terpisah (web_*.png tidak ada di repo; gunakan `docs/` + commit visual sebelumnya).

---

## A. Screen inventory (Legacy WebView2)

| # | Screen / route | Tujuan bisnis | Komponen utama | Data ditampilkan | Event user | State | API/backend | Error / Loading / Empty / Offline / Permission | perilaku khusus |
|---|---|---|---|---|---|---|---|---|---|
| 1 | Discover (Home) — `#view-discover` (active by default) | Discoveriaan: radio mix, jump-back, recommendations | `home-ambient` (2 layer float), `home-hero`: "For you" (Radio button + regen mixes), "Jump back in" row, `home-hero-dots`; `sp-home` promo: orbiting platform logos + "For you"/"Home"/"Trending now"/"Favorite artists" rows; right side: search-results-container | Pos, title, artist, album, cover, album/artist links, vibe play lists | Play track (hero/carousel), regen mixes (`btn-regen-mixes`), "Open Integrations" promo | Home store; `home-for-you`, `home-trending`, `home-quick`; toast on errors | electronAPI: stream info, WEB_REMIX search, `rescanLocalMetadata` | Skeleton: `skeleton-shimmer` (sp-row loading) | Dots carousel (`home-hero-dots`) clickable → focus row; orbit ring rotates `riff-orbit`; promo dismiss |
| 2 | Search — `#search-results-container` | Search tracks across platforms | filter chips (All/Songs/Artists/Albums/Playlists), artist card container, albums container, playlists container, tracks list | Result query, artist cards, album cards, playlists, tracks | `trigger_search` — debounce on Enter?; platform chips (`cta.has-hover`); play/like/add-playlist on result rows | Search store, UI store | Axum search endpoint via server port; yt-dlp/InnerTube fallback | Empty state on 0 results; loading via placeholders | Search input clears on empty; filter chips have hover; `search-filter-chips` arrow |
| 3 | Liked Songs — `#view-favorites` | Manage liked library | `playlist-detail-header` hero (heart cover, "Liked Songs", "N songs", play, shuffle, more), toolbar: sort select (manual/added-desc/asc), search box, recs section | Favorites tracks, recs (favorites-recs) | Play, shuffle, add-to-playlist, like (toggle), search, sort, refresh recs | Favorites store, UI search store | API: `GET /api/liked` (server), liked toggle via electronAPI | Empty state: "No favorite tracks yet…" + search box hidden; loading: placeholder rows; offline: disabled play button | "Recommended for you" section appears when recs non-empty; sort dropdown (m3-dropdown) |
| 4 | Offline Library — `#view-local` | Local file library management | Hero (cloud_off cover, "My files", "N tracks in library"), toolbar: play, shuffle, Add files, Add folder, local search box, sort dropdown | Local tracks, albums row, search/sort state | Play, shuffle, add files (`pickLocalFiles`/`importAudioFiles`), add folder (`pickLocalFolder`), search, sort, play | Library store (`localFiles`), scan state | `rescanLocalMetadata`, `importAudioFiles`; library scanner + Lofty in Rust | Empty state: "No local tracks in library yet." with Add files primary buttons; loading: skeletons; offline: action disabled | Local albums row visible when albums exist; size adapts on resize; toast on add success/failure |
| 5 | History — `#view-history` | Recently played | view-header (h2 History + Clear history), tracks list | History tracks | Play, clear history | History store | Server: `GET /api/history` | Empty: "No recently played tracks."; loading: skeleton; offline: empty view | Toast after clear |
| 6 | Playback queue — `#view-queue` | FIFO queue | view-header (h2 Playback queue + Clear queue), tracks list | Queue tracks (`queue_tracks`) | Play, clear queue, play/add/like on rows | Queue store | Server `GET /api/queue` (syncs with play session) | Empty: "Queue is empty…" hint; loading: skeleton; offline: read-only | Toast after clear; queue visually updates when add/next |
| 7 | Playlist detail — `#view-playlist-detail` | View/edit playlist | Hero: back btn, cover (`hero-fx`), type badge "Playlist", title (editable via pencil), desc (inline edit, hidden when empty), count, action bar: play, shuffle, add local, more, download, export JSON, delete; main: search box, sort dropdown (manual/added-desc/asc), table head (#/Title/Album/Plays/Date added/duration), tracks list, recs section | Tracks of playlist; `playlists` list for picker | Play, shuffle (add to next), edit title/desc, rename? , delete, download, export, search, sort, add track via playlist picker | Playlist store, UI store, toast state | Server playlist detail endpoint; playlist CRUD via electronAPI `showPlaylistContextMenu`/`openPlaylist` etc. | Empty playlist: table + "This playlist is empty. Hover any track and click + to add songs."; loading: skeleton; offline: read-only | Toast on rename/delete/export; hero-fx appears when playlist thumbnails available |
| 8 | Album — `#view-album` | Album detail + play | Hero side: cover, title, artist button, count, actions (play, shuffle, save, DM like/dislike, more), daily mix feedback strip; tracks col | Album tracks | Play, shuffle, save (add to album/library), DM like/dislike (Daily Mix feedback), open artist | Album store | Axum album endpoint; local/server: like tracking via `toggle-like` | Empty: "0 tracks" counter; loading: skeleton; offline: read-only | DM feedback button hidden when no feedback data |
| 9 | Artist — `#view-artist` | Artist profile + popular tracks + albums | Artist hero (img, verified cookie badge, follow/block/bio buttons, name, fans line, bio), sections: Popular songs, Albums (shelf), All tracks; artist category detail (Albums/Tracks views) | Artist tracks, albums, fans | Play, follow, block, bio modal, open album/artist category, view all sections, expand "All tracks" | Artist store, category state | Axum artist endpoint; rate limit from InnerTube | Loading: skeleton; offline: read-only | Bio button hidden when no bio; verified badge rotates (`cookieBadgeRotate`) |
| 10 | Settings — `#view-settings` | App settings | `settings-view` with h2 "Settings", `settings-layout` with settings-nav (tabbed) + panels (many setting-items: M3 style, theme, storage, caches, plugins, integrations, banners, about, equalizer, sleep, lyrics, broadcast…) | UI settings, cache size, integrations status, sleep status, album download, discord rpc… | All toggles/switches/sliders; drag zones; copy; verify | UI store, Settings store; storage via storageSaveSync; cache via getCacheSize; Discord RPC via discordRpc* | Errors: toast/log (`logError`), m3ErrFadeIn (error banner); offline: most disabled | Empty states for caches, integrations; toggle transitions; date/time inputs; file pickers via electronAPI | "About" shows version via getAppVersion; update modal (`update-modal`) when update available; "Open Logs Folder"; `data-i18n` localized |
| 11 | Right Panel Lyrics (Now Playing) — `#right-panel` | Lyrics display + track meta | Header actions (Edit, Sync, Translate, Fullscreen karaoke, Word/Line mode toggle, close); artwork box (cover, fallback svg, vinyl overlay spin, change-cover btn); meta (title, artist, save track btn); lyrics section (`lyrics-container`, lazy); warn badge (`lyrics-warn-badge`); credits card (`rp-credits-card`); visualizer canvas (np-bg) | Lyrics lines (LRC), sync data, track meta | Play/pause via playbar; lyric line click (tap); word/line mode toggle; translate (opens modal); sync; edit (opens modal); enter fullscreen | Lyrics store, player state; lyrics cache (RiffLyricsCache files) | Axum lyrics endpoint via server; inner html parser `lyrics.js`; electronAPI getAudioOutputInfo (audio device list) | Loading: skeleton; error: warn badge + retry; empty: callout "Lyrics are now here" when no lyrics; offline: cached lyrics; no-match warning badge | Lyrics highlight follows progress; word/line mode animations (word transition per word with css styles); translate opens modal |
| 12 | Mini Lyrics — `#mini-lyrics-container` | Small lyrics overlay during mini mode | Small lyrics strip | Current line + upcoming lines | Click → open fullscreen lyrics / hide | UI store mini-lyrics visibility | lyrics store | Hidden by default; smooth opacity fade (`m3FadeUp` variants) | Shows only when player plays + lyrics enabled + `lyrics_open` visible |
| 13 | TV Karaoke (Fullscreen) — `#tv-karaoke` | Full-screen karaoke sing-along | TV: ambient blobs (`tvk-ambient-blob` b1/b2/b3), title marquee, orbiting logos ring, lyrics columns, progress bar, word/line highlight, pause playbar | Lyrics lines | Tap line (start sync), play/pause, mode toggle, close | Lyrics store, fullscreen state | Same as right panel | Fullscreen with per-word highlight; marquee scrolls at constant speed; ambient blobs pulse (`tvk-in`/`tvk-out`) | Close button (×) at top center hover; Esc?; restricted drag; resize locked |
| 14 | Welcome Setup — `welcome.html` | Onboarding wizard | Brand (riff logo), card panels, buttons (Primary/Secondary/small) | Setup steps (pasted/workspace dirs, drive, folders) | Next/back/Go | Setup store (localStorage `RIFF_WELCOME_*`) | electronAPI: pickDataDir, pickLocalFiles/pickLocalFolder (folder add), googleImportLibrary (optional), storageSaveSync; getDataDir | Disabled buttons on incomplete steps; closing sets `first-run-done`; toast on errors | Card animation (`cardIn` 0.35s); window buttons (win-controls, fixed top-right); app-region drag on body |
| 15 | Playbar (footer) — `#playbar` | Transport + scrubber + mini mode | Artwork wrapper (m3c cover), title + artist text, scrubber with time labels, volume bar (`#volume-bar`), transport buttons (prev/play/pause/next/repeat/shuffle/loop), EQ bar (`spBarEq`), mini lyrics button, right panel lyrics button, menu icons; `panel-resize-edge` for right panel; top bottom drag zones | Current track meta; position; volume | Transport buttons; scrubber drag + double-click `view-full-tracks`?; volume click-drag; mini toggle; scroll wheel for volume? | Player store, lazy-loaded `audio.js`; state | Server playback endpoints; stream proxy (`GET /api/audio-proxy` range); yt-dlp fallback | No transport → disabled states; buffering indicator (inline spinner); scrubber bounds update on resize | Mouse wheel over scrubber → seek; resize adjusts scrubber width; mini mode max 500px height; smooth BG visualizer opacity toggling |
| 16 | Bulk Select — `#bulk-select-bar` | Multi-select on list views | Count text, Select all, Add to playlist, Like all, Remove (context-dependent), ✕ clear | Selected tracks | Toggle selection per row (hover checkboxes); actions | List store, pending playlist picker, toasts | Server: bulk add/like; local `savePlaylistsJson` | Enabled on hover; stays visible when user scrolls |

**Backend (electronAPI / Tauri) inventory (via tauri-bridge.js):**
`minimizeWindow, maximizeWindow, closeWindow, getServerPort, getAppVersion, openExternal, getDataDir, pickDataDir, setDataDir(no-op), getDownloadDir, pickDownloadDir, setDownloadDir(no-op), resetDownloadDir(no-op), pickMedia, pickLocalFiles, pickLocalFolder, importAudioFiles, rescanLocalMetadata, readCacheFile, writeCacheFile, getGpuCompositing(true), setAppProxy(true), restartApp, googleLogin, googleLogout, googleAccountInfo, googleImportLibrary, googleAuthStatus, youtubePlaybackLogin/Logout/Status/onYoutubePlaybackAuth, onWindowVisibility, discordRpcUpdate/Clear/SetEnabled/GetEnabled/GetConnected/Reconnect, getCacheSize, clearCache, getAudioOutputInfo, checkForUpdates/updateAction/installUpdateNow/onUpdate* (4 listeners), savePlaylistsJson, showPlaylistContextMenu, accountExport, accountImport, logError, openLogsFolder, getDiagnostics, setZoomFactor(no-op), getZoomFactor, clearWebFrameCache(no-op), storageSaveSync, storageSaveBatch, setMediaKeys, setGlobalShortcuts, onMediaCommand, onMediaControl`
(server endpoints in riff-core: `/api/audio-proxy`, `/api/search/...`, stream, library, queue, history, albums, artist, lyrics, stream info; token auth middleware `x-riff-token`).

**Keterkaitan dengan Slint target (riff-slint views):** Discover→HomeView (Trending+For you), Search→SearchView, Liked→FavoritesView, Offline→LocalFilesView, History→HistoryView, Queue→QueueView, Playlist→PlaylistDetailView. Playbar≈RightPanel+Playbar-slint. TV Karaoke≈FullscreenLyricsOverlay (already in slint).

---

## B. UI/visual inventory (Legacy WebView2)

**Global tokens & primitives (from styles.css :root + globals):**
- Colors: `--bg #121212`, `--panel #1e1e1e`, `--panel-brand #181818`, `--el #2a2a2a`, `--el-hover #333333`, `--accent #e8e8e8`, `--on-accent #121212`, `--accent-hover #ffffff`, `--muted #8f8f8f`, `--text #f2f2f2`, `--r 12px`, `--r-sm ~4-6px`, primary purple `#5038a0` (favorites), daily mix `#c8a12b` etc.; artist colors via `--playlist-accent-color`.
- Typography: Inter stack (fallbacks Segoe/SF), default font-size 13-14px, `font-weight 400/500/600/700/800`, all caps for uppercase labels.
- Icons: inline SVG monochrome icons (stroke 1.5-2px, viewBox 0 0 24 24), heroicons style; orbit logos via external CDN (simpleicons); verifier cookie badge via SVG.
- Images: covers (`right-panel-cover`, playlist covers, albums, artist), `assets/*.svg` (route covers, RippleBtn), `assets/liked.png` (favorites hero), `assets/riff.svg`, `assets/rifflibrary.svg`, devsize photos.
- Buttons: `btn-pill-primary/secondary`, `btn-outline-pill`, `btn-icon-pill` (28-36px, rounded), `sp-hero-play-btn` (36px play), `sp-quiet-icon-btn`, `lyrics-action-btn`, `tv-karaoke-close` (hover glow), `m3-dropdown-trigger` (pill 28-32px), `effect-switch-btn` (36px pill switch w/ `.thumb`), `pill-range` sliders, `pill-select` dropdowns, `toggle-switch`s, `info-toggle` (line 2356?), `sp-hero-play-btn`, `delete-btn`.
- Tables: `sp-tracks-table-head` (#, Title, Album, Plays, Date added, duration), track rows (`sp-track-row`, hover bg ~rgba(255,255,255,0.04), duration 0.16s, border-radius 4px, checkbox circle when bulk select), gaps.
- Lists: `tracks-list` containers, `.playlist-recs-section` (+`playlist-recs-header`, `playlist-recs-title/sub`), hero-fx backgrounds (linear-gradient colors), hover scales 1.02→1.04, shadows (`0 4px 16px rgba(0,0,0,0.5)`).
- Modals: `modal-backdrop` (semi-transparent, flex center, focus trap), `settings-card` (`card`-like, radius 16px, white border ~1px #333), `dialog-actions` bottom bar.
- Right panel: fixed right edge, 358px when lyrics open (`panel-resize-edge`), slides (translateX), overlay blur behind (right-panel-bg ~ rgba(28,28,31,0.9)).
- Playbar: 66px fixed bottom, `playbar-bg-visualizer` canvas fade 0.06 opacity, blur (`--playbar-height: 72px`).
- Mini player: `mini-mode` toggles — small pill 32px radius rounded, centered (with `win_mini_player` btn when always-on-top mode)… max height 500px.
- Titlebar: custom buttons (minimize/maximize/close icons, 16px), `titlebar-drag`; height 32px, blurred bottom bar.
- Scrollbars: custom slim dark scrollbars (overflow-y auto), `riff-scroll` (sticky headers? some).
- Focus: `:focus-visible` outlines (`outline: 2px solid #4fc3f7` on some), tabindex on menus; default outline styling otherwise.
- Hover: ~10-15% luminance up on buttons/rows, SVG icon color change to `Theme.text-primary` on hover; card scale; play/pvp buttons popout (`rp-track-save-btn` popout at right side `style="right: -12px"`).

**Theme:** dark only (no light mode for main app; welcome is custom dark).

---

## C. Animation inventory (Legacy WebView2)

> Each entry: name, trigger, props, duration, delay, easing, direction/from→to, repeat/interruption/reduced-motion, gestures.

```yaml
animation: meshFloat
trigger: home-ambient layers on Discover mount; loop (forever)
props:
  - property: transform.translateY
    from: -20px → to: 0px → cycle
  - property: transform.scale
    from: 1.0 → to: 1.15 → cycle
duration: 18000ms
delay: 0ms
easing: linear
interruptible: true (hover pause handled via noop css var)
reduced_motion: transform still moves, but max scale 1.0 (guarded)
gesture: none
acceptance:
  - 2 animated layers at 120ms + 15s offset
  - pauses when window unfocused? no, always runs (cheap shaders)
```

```yaml
animation: homeInAnim
trigger: Discover view mount (discover-home visible)
props:
  - opacity: 0 → 1
  - transform.translateY: 20px → 0px
duration: 500ms
easing: cubic-bezier(0.4, 0, 0.2, 1)
direction: top-down cascade
interruptible: true
gesture: none
```

```yaml
animation: riffViewSubtleIn / riffElemCascadeIn
trigger: generic view/element mount on Discover, History, Favorites, Local, PlaylistDetail, Artist, Album, Settings, Search results
props:
  - opacity: 0 → 1
  - transform.translateY: 8px → 0px
duration: 320ms
delay: stagger 0 → 200ms (element index)
easing: ease-out (cubic-bezier)
direction: bottom-up, staggered
interruptible: true (new mount resets)
gesture: none
```

```yaml
animation: riffCascadeSlideIn / riffTrackRowSlideIn / riffTrackRowSlideOut
trigger: track row add/remove (library scan finish, queue changes, search results); nav between tab content
props:
  - opacity: 0 → 1 / 1 → 0
  - transform.translateX: -16px → 0 (enter), 16px → 0 (leave, inverted)
  - offset props for staggered enter: 0 → 30ms
duration: 240ms enter, 180ms leave
easing: ease-out (in), ease-in (out)
interruptible: true; exit replaced by opacity-out if element remains
gesture: none
```

```yaml
animation: sbRowFadeIn
trigger: rail/drawer list rows on rail load, playlist drawer open
props:
  - opacity: 0 → 1
  - transform.translateY: 6px → 0
duration: 360ms
easing: ease
interruptible: true
gesture: none
```

```yaml
animation: cpmSlideIn / cpmSlideFromRight / cpmSlideFromLeft
trigger: "cpm" promo (Spotify promo strip) on Discover mount/open
props:
  - opacity: 0 → 1
  - transform.translateX: translateX(60px), translateY(30px) scale(0.95) → 0 scale 1 (FromRight), mirrored FromLeft
duration: 420ms ease (cubic-bezier)
interruptible: true
gesture: none
```

```yaml
animation: riff-orbit (bottom "sp" orbiting logos), riff-orbit-counter
trigger: Discover bottom sp-section mount; continuous
props:
  - transform.rotate: 0deg → 360deg (counter), 60deg → 420deg per logo
  - scale: 1 → 1.08 → 1 (offset orbit ring)
duration: 420s base (rotate), loop forever
easing: linear-ish; slight ease
interruptible: true
gesture: none
```

```yaml
animation: tvk-in / tvk-out
trigger: TV Karaoke open (btn-tv-karaoke click) / close (Esc/close btn); also tv-karaoke background blur fade
props:
  - opacity: 0 → 1 / 1 → 0
  - background.blur: 0px → 24px (ambient)
  - transform.scale: 0.96 → 1
duration: 320ms
easing: cubic-bezier(0.2, 0, 0, 1)
direction: expand/shrink; close = reverse
interruptible: false (locked while open, Esc forces close, esc closes reverse animation; background shrinks)
gesture: locked
reduced_motion: instant display
```

```yaml
animation: tvkTitleMarquee
trigger: TV Karaoke title render
props:
  - transform.translateX: 100% → 0 (scroll)
  - loop: continuous translateX (keyframes 0% → 100%)
duration: variable (speed based on title length: 4000ms→8000ms)
easing: linear
gesture: none
```

```yaml
animation: tvk-panel-in
trigger: tvk panel (lyrics) open after tvk-in
props:
  - opacity: 0 → 1
  - transform.translateY: 16px → 0
duration: 260ms
easing: ease-out (cubic-bezier)
direction: bottom-up
interruptible: true
```

```yaml
animation: tvk-pop (notifications inside TV)
trigger: toast popups (`dropletToastIn` clone)
props:
  - opacity: 0 → 1 → 0 (fast snap-in, 400ms out)
  - transform.translateY: -100% → 0 → -100%
duration: 450ms / 400ms
easing: keyframe (smooth start, ease-in stop) / keyframe-reverse
repeat: 1
gesture: none
```

```yaml
animation: tvkOrbit1/2/3/4 (single logo in orbit ring)
trigger: TV Karaoke continuous
props:
  - transform.rotate: fixed angle (0/72/144/216) → +full loop; initial delay staggered (0/400/800/1200ms)
duration: 12000ms loop
easing: linear
gesture: none
```

```yaml
animation: presetsMainFloat
trigger: PresetsDrawer open/close (sp-section floats, `presetsMainFloat`)
props:
  - opacity: 0 → 1
  - transform.translateY: 12px → 0
duration: 280ms
easing: ease-out
interruptible: true
```

```yaml
animation: presetsNoteTg
trigger: presets note toggle (`presetsNoteTg`)
props:
  - transform.translateY/X: stagger (0 → +4px)
duration: 180ms ease-out
gesture: none
```

```yaml
animation: dropletToastIn / dropletToastOut
trigger: toast pop/dismiss (toastRowEnter/Leave also)
props:
  - opacity: 0 → 1 / 1 → 0
  - transform.translateY: -100% → 0 / 0 → -100%
  - transform.scaleY: 0.4/0.8 → 1
duration: 450ms in / 420ms out
easing: keyframe(elastic-start, linear end) / reverse; snap-out ease-in-out reverse
repeat: 1; interruptible: true (replaced by latest toast)
gesture: none
```

```yaml
animation: toastRowEnter / toastRowLeave
trigger: toast queue item add/remove
props:
  - opacity + translateY + scale staggered stagger (0/40/80ms)
duration: 300ms ease
gesture: none
```

```yaml
animation: toastBadgePop
trigger: toast badge show (`toastBadgePop`)
props:
  - transform.scale: 0 → 1 (overshoot)
  - opacity: 0 → 1
duration: 280ms
easing: cubic-bezier(0.17, 0.89, 0.32, 2) (overshoot)
interruptible: true
gesture: none
```

```yaml
animation: skeleton-shimmer
trigger: skeletons on library load/scan, playlist recs loading, playlist detail load, lyrics load
props:
  - background.gradient: shifts l-to-r across skeleton blocks (`--skeleton-bg`)
duration: 1600ms
easing: ease (linear keyframe via CSS var)
repeat: loop (infinite)
gesture: none
```

```yaml
animation: vinylSpin
trigger: vinyl overlay on play start/pause (`vinyl-overlay`)
props:
  - transform.rotate: 360deg/s * duration (45s on play, stops on pause)
duration: relative to play
easing: linear
repeat: loop; reverse: stops on pause
gesture: none
```

```yaml
animation: cacheCheckDraw
trigger: settings cache check result
props:
  - stroke-dasharray: 1→200→90/200→-124 → 0
  - stroke-dashoffset: 0 → -35 → -124
  - animation-duration: 350ms (per stroke reset)
duration: 450ms
easing: ease-in-out
repeat: 1
gesture: none
```

```yaml
animation: connectedExpand / connectedCollapse / connectedContentIn / connectedContentOut
trigger: right panel expand (Lyric panel open)
props:
  - clip-path: inset(100%) → inset(0) / reverse
  - opacity 0 → 1 (fade after expand, or pre-fade before collapse)
  - translateY: +8px → 0
duration: 340ms expand, 280ms collapse
easing: ease-out / ease-in
interruptible: false (locked during transition)
gesture: panel resize
```

```yaml
animation: audioPanelIn
trigger: Audio FX panel open (fx menu)
props:
  - opacity: 0 → 1
  - transform.translateY: 6px → 0
duration: 280ms
easing: ease
interruptible: true
```

```yaml
animation: fxFadeIn
trigger: Effects list popups (Speed, Q) on audio module?
props:
  - opacity: 0 → 1
  - transform.translateX: -8px → 0 (from left) / reverse (exit)
duration: 220ms
easing: cubic-bezier(0.4, 0, 0.2, 1)
gesture: none
```

```yaml
animation: sleepDot (context of Nearform: sleep mode indicator blink)
props: blink cycle, opacity pulse 0.3 → 1
duration: 1000ms loop
gesture: none
```

```yaml
animation: cookieBadgeRotate
trigger: verified badge on artist profile/headings
props:
  - transform.rotate: 360deg continuous
duration: 45s
easing: linear; offset delay +0.5s
gesture: none
```

```yaml
animation: artistPaneSlideIn
trigger: artist hero pane mount
props:
  - opacity: 0 → 1
  - transform.translateX: 60px → 0
duration: 280ms
easing: ease-out
gesture: none
```

```yaml
animation: g-rot / g-dash (generic SVG spinner)
trigger: loading in some panels
props:
  - transform.rotate: 360deg loop / stroke-dasharray cycle
duration: 1000-1600ms
gesture: none
```

```yaml
animation: spBarEq
trigger: mini-mode EQ bar pulse (`spBarEq`)
props:
  - height/scale pulse: 0.6 → 1 (random)
duration: 800ms
easing: ease-in-out
repeat: random 1-2
gesture: none
```

```yaml
animation: riffFingerWave
trigger: ??? (sp-finger-wave icon?) — legacy: used at bottom of sp homepage (finger pointing)
props: translateY pulse
duration: 3000ms loop
gesture: none
```

```yaml
animation: m3FadeUp
trigger: settings-ish modals? generic fade-in
props: opacity + translateY 6px → 0
duration: 160-220ms
easing: ease
interruptible: true
```

```yaml
animation: m3ContainerOpen / m3ContainerClose
trigger: any M3 settings-style container (settings cards, toasts, popups) open/close
props:
  - transform.scale: 0.96 → 1 / 1 → 0.98
  - opacity: 0 → 1 / 1 → 0
  - border.radius: ~10-16px
duration: 160-220ms
easing: ease
direction: scale+fade; close: reverse (shrink)
interruptible: true
```

```yaml
animation: m3Rotate
trigger: sp spinners (sp loader)
props: transform.rotate: 360deg loop
duration: 1600-2000ms
gesture: none
```

```yaml
animation: m3Dash
trigger: loader dash animation (sp loader on Discover? onboarding)
props: stroke-dasharray/offset pulse
duration: 1000ms
gesture: none
```

```yaml
animation: m3CheckPop
trigger: settings confirmation toggles (e.g., on completing device binding?), onboarding steps check
props:
  - transform.scale: 0 → 1 (overshoot, elastic)
duration: 220ms
easing: spring-ish cubic-bezier(0.16, 1, 0.3, 1)
gesture: none
```

```yaml
animation: m3ErrFadeIn
trigger: error banner show (settings/toast area)
props:
  - opacity: 0 → 1
  - transform.translateY: +6px → 0
duration: 220ms ease
direction: bottom-up
gesture: none
```

```yaml
animation: m3SidebarTabJelly
trigger: rail (left sidebar) tab buttons hover
props: transform.scaleX pulse 1 → 1.1 → 1
duration: 180ms ease
gesture: hover
```

```yaml
animation: m3CookieRotate (verified badge, on artist pages)
duration: 45s; continuous
```

```yaml
animation: lyricsWarnAutoFade / losslessWarnFadeOut
trigger: lyrics warning badge on lyrics load, and dismiss (5s)
props: opacity pulse → fade out
duration: 2500ms auto-fade
gesture: none
```

```yaml
animation: shuffle-on-indicator (css rp-shuffle-active?); also play-loop etc. — legacy indicator uses rp-shuffle-btn glow pulses (check: uses class toggles; representation: opacity pulse on button while active)
trigger: repeat/shuffle toggle ON states
props: opacity 0.8 ↔ 1 pulse ~800ms loop
duration: 800ms
easing: ease-in-out
gesture: none
```

```yaml
animation: welcome.cardIn (setup card)
trigger: Welcome wizard open
props:
  - opacity: 0 → 1
  - transform.translateY: 10px → 0
duration: 350ms cubic-bezier(0.2, 0, 0, 1)
easing: cubic-bezier(0.2, 0, 0, 1)
gesture: none
```

```yaml
animation: karaoke-tat-in (tvk mar breaking?)
trigger: TV Karaoke text marquee on title (tvkTitleMarquee equivalent), app startup? — covered separately
props: translateX loops, speed 50px/s
gesture: none
```

```yaml
animation: scrollbar-fade (ease for scrollbars)
trigger: scroll on any scrollable
props: opacity 0.35→0.9 (custom scrollbars via webkit), duration 300ms
easing: ease
gesture: scroll
```

**Animation inventory notes:**
- Global interruptible patterns: fades ease 250-350ms; translates ease-out cubic-bezier.
- Staggered: tracks/rows cascade 0-200ms stagger.
- Interruptible during nav: clicking another view immediately animates, canceling previous (keyframes hard-cancel; fade-outs don't await).
- Reduced-motion: no global listener in styles (spotlight uses guarded var for orbit float, but main transitions still run); mention for Slint migration (add OS detection).
- Gestures: scroll on lists (sticky headers excluded?), hover-based color changes (instant), TV Karaoke lock, scrubber drag.
- Accessibility: no reduced-motion honoring except orbit; keyboard-accessible dropdowns; contrast check ~4.5:1 minimum via design tokens.

---

*Inventory written to `docs/migration-inventory.md` — draft (covers: A. Screen inventory with routes, components, data, events, state, API, error/loading/empty/offline/permission behavior; B. Visual inventory: tokens, components, buttons, tables, modals, scrollbars, focus states; C. Animation inventory with ~40 YAML entries: name/trigger/properties/duration/delay/easing/direction/interruptibility/reduced-motion/gestures).
*End of inventory.*
