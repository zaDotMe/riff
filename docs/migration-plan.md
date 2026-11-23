# Migration Plan — Legacy WebView2 → Rust + Slint (riff)

> **Status:** Fase 0 (inventory) + Fase 1 (arsitektur) — siap konfirmasi user sebelum Fase 4 (per-screen migration).
> Terlampir: `docs/migration-inventory.md` (audit), `docs/migration-plan.md` (ini).

## 1. Rencana Arsitektur (Fase 1)

### 1.1 Struktur project Rust + Slint

```
riff/
├── src/                          # LEGACY — frozen (Tauri 2 WebView2 mode "web")
│   └── renderer/                 # HTML/CSS/JS legacy app
├── rust/
│   ├── riff-core/                # EXISTING — Axum backend: db.rs, library.rs, stream.rs, lyrics.rs
│   └── riff-slint/               # TARGET — Slint native UI (existing, being completed)
│       ├── build.rs
│       ├── src/lib.rs            # run(), app state, backend bridge
│       ├── src/domain/           # NEW — business logic (modules per domain)
│       ├── src/services/         # NEW — API client, storage, auth, telemetry
│       ├── src/actions/          # NEW — reducer/action pattern per domain
│       └── ui/
│           ├── app.slint         # MainWindow: titlebar, playbar, right panel, navigation
│           ├── views.slint       # HomeView, SearchView, FavoritesView, LocalFilesView, HistoryView,
│           │                     # QueueView, PlaylistDetailView (ALL migrated screens)
│           ├── components.slint  # TrackRow, HomeCard, Topbar, TableHeader, PillBtn, IconBtn,
│           │                     # PillRange, M3Dropdown, Toast, Skeleton, ProgressBar, MiniPlayer,
│           │                     # SearchInput, EmptyState, SettingsModal
│           ├── modals.slint      # PresetsDrawer, CreatePlaylistModal, AddToPlaylistModal,
│           │                     # FullscreenLyricsOverlay, CustomLyricsModal, SyncLyricsModal,
│           │                     # AudioEditorModal
│           ├── lyrics.slint      # LyricsPanel (word/line mode, karaoke highlight, credits)
│           ├── karaoke.slint     # TV Karaoke (ambient blobs, title marquee, orbiting ring)
│           ├── home.slint        # Discover/Home (For you, Trending, Jump back, Spotify promo)
│           ├── search.slint      # Search view (3 platform chips)
│           ├── settings.slint    # SettingsPanel + modals (M3 style restructure)
│           ├── playbar.slint     # Transport + scrubber + EQ + mini-mode
│           ├── artist.slint      # ArtistView ( Popular/Albums/All tracks, bio, follow )
│           ├── theme.slint       # Design tokens
│           └── motion.slint      # Duration/easing/key motion tokens + interactive states
├── src-tauri/                    # LEGACY shell (frozen) — dual launcher stays
└── docs/
    ├── migration-inventory.md    # Fase 0 (selesai)
    └── migration-plan.md         # this document
```

### 1.2 Pembagian Layer

| Layer | Path | Responsibility |
|---|---|---|
| `ui/` | `riff-slint/ui/*.slint` | Presentation only: layout, animation, hit-testing, event dispatch. **Zero business logic** — reads `in`/`in-out` state from Rust, fires `callback`s. |
| `domain/` | `riff-slint/src/domain/*.rs` | Business logic: playlist model/CRUD, liked/favorites, history, queue, lyrics LRC parsing, trim CapCut port, A-B repeat, speed control, daily-mix (DM) feedback state. Pure Rust, unit-testable. Reuses `riff-core/src/lyrics.rs` for LRC. |
| `services/` | `riff-slint/src/services/*.rs` | Backend integration: Axum HTTP client (token auth middleware `x-riff-token`), storage adapter (SQLite WAL via riff-core), auth launcher (google login, youtube playback), telemetry (`logError`), clipboard (interop), native dialogs (reuse electronAPI bridge for pickers). |
| `platform/` | `riff-slint/src/platform/*.rs` or via electronAPI bridge | Windows capabilities: tray, Discrd RPC, global media keys, volume OS API, zoom, window controls (already in Tauri). |
| `actions/` | `riff-slint/src/actions/*.rs` | Command/reducer pattern per domain: `PlayTrack`, `QueuePush`, `ClearQueue`, `ToggleLike`, `AddToPlaylist`, `DeletePlaylist`, `ClearHistory`, `ToggleTrim`, `SyncLyrics`, `OpenCustomLyrics`, etc. Each maps 1:1 to a Slint `callback`. |
| `tests/` | `riff-slint/tests/`, `riff-core/tests/` | Unit (`#[test]`), integration (RiffSlintApp smoke), visual regression. |

### 1.3 Ownership State — Slint vs Rust

- **Rust owns all business state.** Slint exposes `in`/`in-out` projections of Rust state, `callback`s for actions.
- **Slint owns transient UI state only:** hover/focus, modal open, toast queue, drag states, preview buffers (trim preview), suggestion lists, carousel focus index, scroll snap target.
- Mapping highlights:

| Legacy state (state.js / Tauri bridge) | Slint target |
|---|---|
| `player.currentTrack`, `isPlaying`, `posMs`, `duration`, `volume` | Root `in-out property` + transport callbacks → Slint → Rust `PlayTrack`, `SeekTo`, `SetVolume` actions |
| `queue_tracks`, `play_session_id` | Root `in-out property <[TrackData]> queue` |
| `favorites` (liked), `favorite_tracks` | Root `in-out property <[TrackData]> favorite_tracks` + `callback toggle_like` |
| `history` (recently played, ≤500) | Root `in-out property <[TrackData]> history_tracks` + `callback clear_history` |
| `playlists` (custom), `saved_albums` | Root `in-out property <[PlaylistData]> playlists` + `callback delete_playlist`, `play_playlist_all`, `rename` |
| `library` (local files + album map + scan progress) | Root `in-out property <[TrackData]> local_files` + `ScanProgress` in Rust |
| `search_query` + results per platform | Slint `in-out search_query` (debounce 300ms) + `in-out search_results` bound to Rust `SearchResultsService` |
| Right Panel lyrics: word/line mode, karaoke line, warn badge, credits | Slint in-out state (transient) |
| Toast queue (`dropletToastIn/Out`, `toastRowEnter/Leave`) | Slint: `out property <[Toast]> toast_queue` + transition CSS (`visible`/`opacity` on each Toast component) |
| Theme/zoom/mini-mode/trim-rect/trim-preview | Slint root `in-out` props |
| `ui_mode` DB switch + fallback | Bridge-level (already wired in `src-tauri/src/main.rs`) |

### 1.4 Callbacks Slint → Rust (via electronAPI or direct invoke)

Play/pause/next/prev/repeat/shuffle/seek/volume/speed/pitch/eq/trim/fullscreen/custom-lyrics/sync/audio editor → Slint callbacks → actions on riff-core player (existing Axum endpoints) or direct Tauri commands.
Library: `pickLocalFiles`, `pickLocalFolder`, `importAudioFiles`, `rescanLocalMetadata` — reuse electronAPI bridge as-is.
Auth: `googleLogin`, `youtubePlaybackLogin`, `discordRpc*`, `openExternal` — reuse bridge.
Backup/analytics: `savePlaylistsJson`, `accountExport/Import`, `logError` — reuse.

### 1.5 Async & Threading

- **Tokio runtime** (Tauri async_runtime). All network/disk/DB ops async — UI never blocks.
- Slint callbacks: `callback` → `async fn` (fire-and-forget via channels).
- Rust → Slint updates: Two-way binding / property set on UI thread from async tasks (Tauri main loop).
- Playback: sync via existing riff-core player (WASAPI/symphonia — async-safe).
- Slint UI thread: animation + rendering; no blocking code in UI files.

### 1.6 Error Handling

- `ApiError` → `Error` UI state + toast (`dropletToastIn`) + retry (3× backoff).
- Empty/loading/offline/permission states: Slint `EmptyState`/`Skeleton` per screen (inventory §A).
- Destructive actions: confirm modals (Create/Delete/Remove-from-playlist).
- Logging: `electronAPI.logError` → `riff-core` diagnostics folder + `openLogsFolder`.

### 1.7 Compatibility with WebView2 (dual launcher)

- Fase 0–1: Slint mode fully decoupled; fallback `ui_mode="web"` stays (already wired).
- Web-only features: reviewed per-item; anything needing legacy-only web capability is logged in `docs/migration-web-features.md` with alternatives. Current known gaps: OAuth browser login (reuse `openExternal` + server flow), Google Cast-ish share (openExternal), audio seek precise preview during play (implementable).

## 2. Design System (Fase 2)

Design tokens in `theme.slint` — tokens over values:

| Token | Value |
|---|---|
| Colors: `primary #5038a0`, `primary-hover #6a4ece`, `bg #0d0d0d`, `panel #121212`, `text-primary #f2f2f2`, `text-secondary #b3b3b3`, `text-muted #6b7280`, `error #f44336`, `success #4caf50`, `warning #ff9800`, `danger #f28b82`, `radius-lg 16px`, `radius-xl 20px`, `radius-sm 4px` |
| Spacing: `space-1 4px ... space-12 48px` (multiples of 4) |
| Radius: `r-lg 16px, r-md 12px, r-sm 6px, r-xs 2px` |
| Shadow: `shadow-sm 0 2px 8px #0008`, `shadow-md 0 8px 24px #000a` |
| Type: Inter stack, size 13–15px, weights 400/500/600/700/800 |

Core components (mapped to legacy):
`TrackRow`, `HomeCard`, `Topbar`, `TableHeader`, `PillBtn`, `IconBtn`, `PillRange` (slider), `M3Dropdown` (matte glassy selects), `Toast`, `Skeleton`, `ProgressBar`, `MiniPlayer` (auto max-500px, center + win btn), `EmptyState`, `SettingsModal`, `SettingsSidebar`, `SearchInput` (two-way), `TrimWave` (port of CapCut wave), `Tooltip`, `Badge` (cookie/verified, chips), `DailyMixFeedback`.

Hover/focus/pressed/disabled states defined per token-driven CSS class system. Keyboard navigation on all interactive elements (`tabindex`), `aria-label` on every interactive element.

## 3. Motion System (Fase 3)

- Tokens (`motion.slint`): `duration-instant 0, fast 120ms, normal 200ms, medium 300ms, slow 400ms`; `easing-standard cubic-bezier(0.4, 0, 0.2, 1)`, `easing-decelerate cubic-bezier(0.25, 0, 0, 1)`, `easing-accelerate cubic-bezier(0.2, 0.8, 0.2, 1)`, `easing-emphasized cubic-bezier(0.23, 0, 0, 1)`.
- Global patterns: panel enter `fadeIn + translateY(8px)`, exit = reverse; nav transitions = reverse; list rows = stagger 0→150ms + fade+translateX(-8px)→0, exit opacity-0 only; modals = backdrop `fade 0→0.45` + content `scale 0.98→1 + fade`, close = reverse (scale→0.98, opacity→0).
- Reduced motion: `root.prefers-reduced-motion` → crossfade-only mode (150ms, no loops).
- Performance: ≥60 FPS target on Windows; animations on CSS-transform/opacity only; 64-bit Rust animate on UI thread.

## 4. Fase 4 — Migration per screen (order: highest value first)

Each screen follows: `docs/migration-screen-<name>.md` with analysis/plan/verify (Fase 4 format).

| # | Screen | Slint target | Migration notes |
|---|---|---|---|
| 1 | **Discover/Home** | `home.slint` + `components.slint: HomeCard/TrackRow/EmptyState/ProgressBar` | For you (Radio + refresh), Trending, Jump back, Spotify promo orbit (external CDN + `prefers-reduced-motion`), dots carousel, hover scale. Test with empty states (trending/for-you). |
| 2 | **Search** | `search.slint` | 3 platform chips (custom pills, tabs + Tooltip), debounce 300ms, `search-platform` two-way binding to Rust, platform-changed → `trigger_search`. |
| 3 | **Liked Songs** | `FavoritesView` | Manual reorder (drag-drop via Slint TouchArea, legacy used `.tracks-list.draggable`), inline search, sort (legacy: `added-desc/asc`), "Recommended for you" section + refresh, like toggle. |
| 4 | **Offline Library** | `LocalFilesView` | Scan progress indicator, folder picker via bridge, local albums row (two-col grid), size-adaptive, plays. |
| 5 | **History** | `HistoryView` | Clear history button, timestamp grouping (today/yesterday/week/month — legacy renders flat). |
| 6 | **Playback queue** | `QueueView` | Live sync with play session (`QueuePush`/`QueueClear` actions), play/pause from rows, toast on empty. |
| 7 | **Playlist detail** | `PlaylistDetailView` | Edit title/desc inline (legacy did pencil rename), delete playlist + confirm modal, Export JSON, Download all, **A-B trim port (CapCut wave, Slint Canvas + audio engine clamp)** — important per summary. |
| 8 | **Album** | `ArtistView` (details) | Legacy view shows pages; Slint: full-bleed album sheet via `Modal` or in-page — decide: use DetailModal variant (smaller than playlist detail, no desc). |
| 9 | **Artist** | `artist.slint` | Popular/Albums/All tracks, bio (Markdown → Slint Text), verified badge, follow/block, "All tracks" expand, category views (Albums/Tracks). |
| 10 | **Settings** | `settings.slint` (existing) + M3 restructure (done earlier) | Categories from legacy nav, all toggles/sliders bound to Slint state; cache size/clear, Discord RPC section, Export/Import account, language selector (data-i18n). |
| 11 | **Right Panel Lyrics + Playbar** | `lyrics.slint` + `playbar.slint` | Word/Line mode, real-time highlight, LRC parser, translate modal, sync modal, credits card, visualizer canvas + "save to lyrics" (from playbar). |
| 12 | **TV Karaoke** | `karaoke.slint` (exists) + enhancements | Fullscreen marhquee + orbiting ring + ambient blobs; edge resize toggling (legacy hides caption during play), focus-visible improvements, 30fps highlight. |
| 13 | **Custom Lyrics** | `CustomLyricsModal` (exists) | TextArea-less input: Slint TextInput row list + paste; quote handling per legacy. |
| 14 | **Sync Lyrics** | `SyncLyricsModal` (exists) | Tap line to set, Space to jump next, Save; use Rust LRC save. |
| 15 | **Audio FX / Speed/Pitch** | `AudioEditorModal` (exists) + `playbar.slint` | Slider full-width, play/pause display button; pitch/speed routed to Rust via actions. |
| 16 | **Welcome wizard** | New `WelcomeScreen` (slint) | Migration semantics; first-run gate `ui_mode="slint"`? (Leave: optional — flag for next phase.) |

## 5. Verification (Fase 4+5)

1. `cargo test --workspace` — unit + integration (riff-core, riff-slint).
2. **Visual regression**: `shot_views.ps1` → `slint_*.png`; diff vs legacy `web_*.png` + prior slint series; SSIM ≥ 85% per screen (tools: ImageMagick `compare`).
3. **Interaction test** (manual + autohotkey script?): play/pause, queue, playlist CRUD, lyrics sync, search, all shortcuts (`onMediaCommand`/`onMediaControl`).
4. **Animation timing test**: screenshot at slint animation frames vs legacy (~90% match tolerance).
5. **Performance test**: Slint logger → 60 FPS target; leak test: 12h run → ≤5% memory growth.
6. **Offline/error mode**: empty/error states match inventory.
7. **Memory/leak test**: long-run stability.
8. **Security audit**: Windows service perms; clipboard paste interop; capabilities review (`src-tauri/capabilities/default.json`).

## 6. WebView2 Compatibility Notes (to implement)

- `welcome.html` → `WelcomeScreen` in slint (phase: after screen 1).
- `rp-track-save-btn` popout → Slint popout variant (`PopoutBtn` component).
- TV Karaoke ambient → Slint Canvas layers.
- Custom scrollbar fade → Slint `CustomScroll` (all scrollables).
- Ensure NO WebView2 deps left: screens must be covered by Slint (except staged gaps: Google OAuth login, Google Cast, drag-drop media to window — marked as future).

## 7. Rollback per Screen

- Dual launcher: `ui_mode` DB switch — any Slint screen rolls back via `ui_mode="web"` + app restart (no state loss, legacy WebView2 keeps all legacy state).
- Migration initial copy: `migrate_from_legacy.rs` — favorites → Liked, local → riff-core, history, queue → resume, theme/zoom → slint root props, playlists JSON → saved_playlists.json format.

## 8. Fase 5 — Deliverables (per screen iterasi)

For each screen, write `docs/migration-screen-<name>.md` containing:
1. **Analysis**: reference(s), component tree, state diagram, event flow, API contract, animation inventory excerpts.
2. **Implementation plan**: files created/changed (`.slint`, `src/domain/*`, `src/services/*`, `src/actions/*`), Slint components, Rust models, callback/binding map, test plan, complexity estimate (S/M/L).
3. **Implementation**: clean `.slint` files + Rust code + all loading/empty/error/success/partial states + animations.
4. **Verification**: visual/interaction/animation/performance checklist + pass/fail.
5. **Acceptance criteria**: all 10 criteria from legacy inventory must be satisfied.
6. **Notes**: intentional differences + alternatives.

## 9. End of Fase 0/1 — STATUS

✅ Fase 0: `docs/migration-inventory.md` — audit A/B/C complete.
✅ Fase 1: `docs/migration-plan.md` — architecture approved.

**Next step — awaiting user confirmation to begin Fase 4 (screen 1: Discover/Home migration).**
