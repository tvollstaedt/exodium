import { invoke } from "@tauri-apps/api/core";

export interface Game {
  id: number | null;
  title: string;
  sort_title: string | null;
  platform: string;
  developer: string | null;
  publisher: string | null;
  release_date: string | null;
  year: number | null;
  genre: string | null;
  series: string | null;
  play_mode: string | null;
  rating: number | null;
  rating_votes: number | null;
  description: string | null;
  notes: string | null;
  source: string | null;
  application_path: string | null;
  dosbox_conf: string | null;
  status: string | null;
  region: string | null;
  max_players: number | null;
  language: string;
  shortcode: string | null;
  available_languages: string | null;
  /** Titles of the other language variants, unit-separated. Present only on
   *  merged multi-language rows; lets a local search match a localized title. */
  variant_titles: string | null;
  torrent_source: string | null;
  in_library: boolean;
  installed: boolean;
  favorited: boolean;
  game_torrent_index: number | null;
  gamedata_torrent_index: number | null;
  download_size: number | null;
  has_thumbnail: boolean;
  dosbox_variant: string | null;
  /** SHA-256(normalized title)[:16] - filename stem for the bundled or
   *  content-pack thumbnail. Null when no title was available at DB-build
   *  time (very rare). Frontend builds `<preview_dir>/${thumbnail_key}.jpg`. */
  thumbnail_key: string | null;
  manual_path: string | null;
  last_played: string | null;
}

export interface GameList {
  games: Game[];
  total: number;
}

export async function getGames(
  page?: number,
  perPage?: number,
  query?: string,
  genre?: string,
  sortBy?: string,
  collection?: string,
  favoritesOnly?: boolean,
  playlistId?: number | null
): Promise<GameList> {
  return invoke("get_games", { page, perPage, query, genre, sortBy, collection, favoritesOnly, playlistId });
}

export async function toggleFavorite(id: number): Promise<boolean> {
  return invoke("toggle_favorite", { id });
}

export async function cancelDownload(id: number): Promise<void> {
  return invoke("cancel_download", { id });
}

export async function getGenres(collection?: string): Promise<string[]> {
  return invoke("get_genres", { collection });
}

export async function getSectionKeys(
  sortBy?: string,
  query?: string,
  genre?: string,
  collection?: string,
  favoritesOnly?: boolean,
  playlistId?: number | null,
): Promise<string[]> {
  return invoke("get_section_keys", { sortBy, query, genre, collection, favoritesOnly, playlistId });
}

// ── Playlists ────────────────────────────────────────────────────────────────

export interface Playlist {
  id: number;
  name: string;
  /** "curated" (shipped with the catalog, read-only) or "user". */
  kind: "curated" | "user";
  description: string | null;
  game_count: number;
}

export async function getPlaylists(): Promise<Playlist[]> {
  return invoke("get_playlists");
}

export async function createPlaylist(name: string): Promise<number> {
  return invoke("create_playlist", { name });
}

export async function renamePlaylist(id: number, name: string): Promise<void> {
  return invoke("rename_playlist", { id, name });
}

export async function deletePlaylist(id: number): Promise<void> {
  return invoke("delete_playlist", { id });
}

export async function setPlaylistMembership(
  playlistId: number,
  gameId: number,
  member: boolean,
): Promise<void> {
  return invoke("set_playlist_membership", { playlistId, gameId, member });
}

export async function getGamePlaylists(gameId: number): Promise<number[]> {
  return invoke("get_game_playlists", { gameId });
}

export async function getThumbnailDir(collection: string): Promise<string> {
  return invoke("get_thumbnail_dir", { collection });
}

export async function getGameVariants(shortcode: string, collection: string): Promise<Game[]> {
  return invoke("get_game_variants", { shortcode, collection });
}

export async function getInstalledGames(): Promise<Game[]> {
  return invoke("get_installed_games");
}

export async function getRecentlyPlayed(limit?: number): Promise<Game[]> {
  return invoke("get_recently_played", { limit });
}

export interface GameSettings {
  /** "staging" forces DOSBox Staging for an ECE game; null = eXo's choice. */
  engine: string | null;
  glshader: string | null;
  fullscreen: string | null;
  cycles: string | null;
  custom_conf: string | null;
}

export async function getGameSettings(id: number): Promise<GameSettings> {
  return invoke("get_game_settings", { id });
}

export async function setGameSettings(
  id: number,
  engine: string | null,
  glshader: string | null,
  fullscreen: string | null,
  cycles: string | null,
  customConf: string | null,
): Promise<void> {
  return invoke("set_game_settings", { id, engine, glshader, fullscreen, cycles, customConf });
}

export async function getGame(id: number): Promise<Game | null> {
  return invoke("get_game", { id });
}


export async function launchGame(id: number): Promise<string> {
  return invoke("launch_game", { id });
}

/** Whether the game's printing features will be missing at launch (13 eXoDOS
 *  titles enable a virtual printer; Staging has none yet). The backend decides
 *  with the same engine-selection logic launch_game uses, so Windows + an
 *  installed ECE build correctly answers false. */
export async function gamePrintingUnavailable(id: number): Promise<boolean> {
  return invoke("game_printing_unavailable", { id });
}

export interface GameEngineInfo {
  /** Could ECE run this game here at all? Decides whether the emulator choice
   *  is worth offering. Deliberately blind to the user's override, or the
   *  control would disappear the moment they pick Staging. */
  ece_available: boolean;
  /** What will actually run it, override included: engine label, shader note,
   *  printing note. ECE has no shader pipeline, so a CRT setting on an ECE
   *  game is dropped at launch rather than applied. */
  uses_ece: boolean;
}

/** Same engine selection launch_game acts on, so it answers false on Windows
 *  until the ECE build has been extracted, and always false elsewhere. */
export async function gameEngineInfo(id: number): Promise<GameEngineInfo> {
  return invoke("game_engine_info", { id });
}

/** Whether the emulator a Win9x game needs (DOSBox-X / 86Box) is resolvable
 *  on this machine. Backend answers with the launcher's own resolver, so the
 *  panel note can never disagree with an actual launch. */
export async function win9xEngineAvailable(variant: string | null): Promise<boolean> {
  return invoke("win9x_engine_available", { variant });
}

export interface LayoutMigration {
  /** Folder names still holding games, relative to the data dir. */
  folders: string[];
  bytes: number;
  /** False once declined - Settings still offers the merge. */
  prompt: boolean;
}

/** Old per-collection folders that should be merged into the single root. */
export async function pendingLayoutMigration(): Promise<LayoutMigration | null> {
  return invoke("pending_layout_migration");
}

export interface MergeTally {
  moved: number;
  deduped: number;
  skipped: number;
}

/** Moves them into the single root; reports what it did. */
export async function migrateLayout(): Promise<MergeTally> {
  return invoke("migrate_layout");
}

/** Remembers that the user declined the merge. */
export async function skipLayoutMigration(): Promise<void> {
  return invoke("skip_layout_migration");
}

export interface Win9xNetworkStatus {
  enabled: boolean;
  can_enable: boolean;
  detail: string;
  manual_hint: string | null;
}

/** Whether eXo's remote-multiplayer titles can reach their IPX gateway. */
export async function win9xNetworkStatus(): Promise<Win9xNetworkStatus> {
  return invoke("win9x_network_status");
}

export interface Win9xMultiplayerInfo {
  multiplayer: boolean;
  state: "ready" | "needs_permission" | "needs_wired" | "unsupported";
  prompt: boolean;
}

/** What online play looks like for this game on this machine. */
export async function win9xMultiplayerInfo(id: number): Promise<Win9xMultiplayerInfo> {
  return invoke("win9x_multiplayer_info", { id });
}

/** Remembers that the multiplayer question should not be asked again. */
export async function dismissWin9xNetworkPrompt(): Promise<void> {
  return invoke("dismiss_win9x_network_prompt");
}

/** Asks the OS for packet-capture permission via its own auth dialog. */
export async function enableWin9xNetwork(): Promise<Win9xNetworkStatus> {
  return invoke("enable_win9x_network");
}

/** Hands the permission back - same dialog, opposite direction. */
export async function disableWin9xNetwork(): Promise<Win9xNetworkStatus> {
  return invoke("disable_win9x_network");
}

export interface Win9xSupportStatus {
  phase: "ready" | "downloading" | "missing" | "failed";
  progress: number;
  /** Size of utilWin9x.zip; 0 when the torrent index is unavailable. */
  total_bytes: number;
}

/** State of the shared Win9x support files (OS parent images + emulators),
 *  scoped to the tree the given variant boots from when one is passed. */
export async function getWin9xSupportStatus(variant?: string | null): Promise<Win9xSupportStatus> {
  return invoke("get_win9x_support_status", { variant });
}

export async function getConfig(key: string): Promise<string | null> {
  return invoke("get_config", { key });
}

export async function setConfig(key: string, value: string): Promise<void> {
  return invoke("set_config", { key, value });
}

// Opens a manual in the system viewer. Path validation happens in Rust
// (must be under the data dir), so no broad opener capability is needed.
export async function openManual(path: string): Promise<void> {
  return invoke("open_manual", { path });
}

export async function setSeedingEnabled(enabled: boolean): Promise<void> {
  return invoke("set_seeding_enabled", { enabled });
}

export interface TransferStats {
  download_bps: number;
  upload_bps: number;
  uploaded_bytes: number;
  /** Connected peers across all collections - the readout that shows liveness
   *  when the rates are zero. */
  peers: number;
  /** False when no torrent is live - distinct from a live 0 B/s. */
  active: boolean;
}

export async function getTransferStats(): Promise<TransferStats> {
  return invoke("get_transfer_stats");
}

/** Transfer caps in KB/s; `null` means unlimited. */
export async function setRateLimits(upKbps: number | null, downKbps: number | null): Promise<void> {
  return invoke("set_rate_limits", { upKbps, downKbps });
}

export interface TorrentInfo {
  name: string;
  file_count: number;
  total_size: number;
  metadata_size: number;
}

export interface DownloadProgress {
  file_index: number;
  file_name: string;
  downloaded_bytes: number;
  total_bytes: number;
  progress: number;
  finished: boolean;
  installed: boolean;
  error: string | null;
  /** "initializing" while librqbit hashes existing on-disk files; on Windows
   *  with thousands of placeholders this can take 5–10 minutes for a 250GB+
   *  torrent before any pieces transfer. */
  torrent_state?: string | null;
  /** 0..1 of whole-torrent progress. During init = validation pass; once live
   *  = cumulative download. Used for the "Validating…" UI status. */
  torrent_progress?: number | null;
  /** 0..1 progress of the game's extras (GameData: manuals, videos, music) -
   *  they keep downloading after the game itself is installed. */
  extras_progress?: number | null;
  extras_done?: boolean | null;
}

export interface SetupStatus {
  phase: string;
  metadata_progress: DownloadProgress | null;
  dosbox_metadata_progress: DownloadProgress | null;
  games_imported: number;
  ready: boolean;
}

export async function getDefaultDataDir(): Promise<string> {
  return invoke("get_default_data_dir");
}

/** True when a folder holds nothing Exodium would recognise as game data.
 *  Used to catch the "I meant to move my library" misreading of Change. */
export async function dataDirIsEmpty(path: string): Promise<boolean> {
  return invoke("data_dir_is_empty", { path });
}

export async function getTorrentInfo(): Promise<TorrentInfo> {
  return invoke("get_torrent_info");
}

export async function setupStart(dataDir: string): Promise<string> {
  return invoke("setup_start", { dataDir });
}

export async function getSetupStatus(): Promise<SetupStatus> {
  return invoke("get_setup_status");
}

export async function setupImport(): Promise<number> {
  return invoke("setup_import");
}

export async function setupFromLocal(exodosPath: string): Promise<number> {
  return invoke("setup_from_local", { exodosPath });
}

export interface ExodosValidation {
  valid: boolean;
  hint: string;
}

export async function validateExodosDir(path: string): Promise<ExodosValidation> {
  return invoke("validate_exodos_dir", { path });
}

export async function initDownloadManager(): Promise<boolean> {
  return invoke("init_download_manager");
}

export async function factoryReset(deleteGameData: boolean): Promise<void> {
  return invoke("factory_reset", { deleteGameData });
}

export async function uninstallGame(id: number): Promise<string> {
  return invoke("uninstall_game", { id });
}

/** Discard saves and every in-game change, then unpack the ZIP again. */
export async function resetGameData(id: number): Promise<string> {
  return invoke("reset_game_data", { id });
}

export async function downloadGame(id: number): Promise<string> {
  return invoke("download_game", { id });
}

export async function getDownloadProgress(id: number): Promise<DownloadProgress | null> {
  return invoke("get_download_progress", { id });
}

export interface CollectionUpdate {
  collection: string;
  current_hash: string;
  latest_hash: string;
  new_game_count: number;
}

export interface CollectionInfo {
  id: string;
  display_name: string;
  torrent_file: string;
  /** Catalogue rows in this collection - shown on the collection shelf. */
  game_count: number;
}

export async function getAvailableCollections(): Promise<CollectionInfo[]> {
  return invoke("get_available_collections");
}

/** `adopt` lets archives on disk ADD games to the library. Only pass it where
 *  the user is asking what is in a folder (Rescan, data-dir change) - a
 *  download drags its piece neighbours in with it, so the automatic scan must
 *  confirm the library, not extend it. */
export async function scanInstalledGames(adopt = false): Promise<number> {
  return invoke("scan_installed_games", { adopt });
}

export async function getLogDir(): Promise<string> {
  return invoke("get_log_dir");
}

export async function openLogFolder(): Promise<void> {
  return invoke("open_log_folder");
}

// ── Content Packs ────────────────────────────────────────────────────────────

export interface ContentPackStatus {
  id: string;
  display_name: string;
  description: string;
  size_bytes: number;
  version: number;
  supersedes: string[];
  available: boolean;
  installed: boolean;
  installed_version?: number;
}

export interface ContentPackProgress {
  phase: string;
  downloaded_bytes: number;
  total_bytes: number;
  progress: number;
  finished: boolean;
  installed: boolean;
  error: string | null;
}

export async function listContentPacks(collection: string): Promise<ContentPackStatus[]> {
  return invoke("list_content_packs", { collection });
}

export async function installContentPack(collection: string, packId: string): Promise<void> {
  return invoke("install_content_pack", { collection, packId });
}

export async function uninstallContentPack(collection: string, packId: string): Promise<void> {
  return invoke("uninstall_content_pack", { collection, packId });
}

export async function getContentPackProgress(
  collection: string,
  packId: string,
): Promise<ContentPackProgress | null> {
  return invoke("get_content_pack_progress", { collection, packId });
}

export async function cancelContentPackInstall(collection: string, packId: string): Promise<void> {
  return invoke("cancel_content_pack_install", { collection, packId });
}

export async function getPreviewDir(collection: string): Promise<string> {
  return invoke("get_preview_dir", { collection });
}

export async function getPosterDir(collection: string): Promise<string> {
  return invoke("get_poster_dir", { collection });
}

export interface GameMetadata {
  manual_path: string | null;
  manual_kind: "pdf" | "txt" | "html" | null;
  /** Full-resolution paths - what the lightbox opens. */
  images: string[];
  /** Cached 160px copies, aligned 1:1 with `images` - what the strip renders.
   *  An entry equals its `images` counterpart when no thumbnail could be made. */
  thumbnails: string[];
}

export interface VideoStatus {
  /** "fetching" | "ready" | "none" | "error" */
  phase: string;
  /** 0..1 while fetching. */
  progress: number;
  total_bytes: number;
  path: string | null;
  error: string | null;
}

/** Start (or join) the fetch of a game's preview video. Returns immediately -
 *  the video is streamed out of the GameData archive, which can take a minute
 *  on a cold torrent. Poll getVideoStatus. */
/** False on a Linux system whose GStreamer cannot build an audio pipeline -
 *  mounting a <video> there wedges the WebKit process and freezes the app. */
export async function videoPlaybackSupported(): Promise<boolean> {
  return invoke("video_playback_supported");
}

export async function startGameVideo(id: number): Promise<VideoStatus> {
  return invoke("start_game_video", { id });
}

export async function getVideoStatus(id: number): Promise<VideoStatus | null> {
  return invoke("get_video_status", { id });
}

export async function cancelGameVideo(id: number): Promise<void> {
  return invoke("cancel_game_video", { id });
}

/** Playable URL for a media file. Linux answers with a localhost HTTP URL
 *  (WebKitGTK cannot play media through the asset protocol); macOS/Windows
 *  answer null - use convertFileSrc there. */
export async function mediaUrl(path: string): Promise<string | null> {
  return invoke("media_url", { path });
}

export async function getGameMetadata(
  collection: string,
  title: string,
  shortcode: string | null,
  manualPath: string | null,
): Promise<GameMetadata> {
  return invoke("get_game_metadata", { collection, title, shortcode, manualPath });
}
