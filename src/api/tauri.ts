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
  torrent_source: string | null;
  in_library: boolean;
  installed: boolean;
  favorited: boolean;
  game_torrent_index: number | null;
  gamedata_torrent_index: number | null;
  download_size: number | null;
  has_thumbnail: boolean;
  dosbox_variant: string | null;
  /** SHA-256(normalized title)[:16] — filename stem for the bundled or
   *  content-pack thumbnail. Null when no title was available at DB-build
   *  time (very rare). Frontend builds `<preview_dir>/${thumbnail_key}.jpg`. */
  thumbnail_key: string | null;
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
  favoritesOnly?: boolean
): Promise<GameList> {
  return invoke("get_games", { page, perPage, query, genre, sortBy, collection, favoritesOnly });
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
): Promise<string[]> {
  return invoke("get_section_keys", { sortBy, query, genre, collection, favoritesOnly });
}

export async function getThumbnailDir(collection: string): Promise<string> {
  return invoke("get_thumbnail_dir", { collection });
}

export async function getGameVariants(shortcode: string): Promise<Game[]> {
  return invoke("get_game_variants", { shortcode });
}

export async function getInstalledGames(): Promise<Game[]> {
  return invoke("get_installed_games");
}

export async function getGame(id: number): Promise<Game | null> {
  return invoke("get_game", { id });
}


export async function launchGame(id: number): Promise<string> {
  return invoke("launch_game", { id });
}

export async function getConfig(key: string): Promise<string | null> {
  return invoke("get_config", { key });
}

export async function setConfig(key: string, value: string): Promise<void> {
  return invoke("set_config", { key, value });
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

export interface UpdateInfo {
  updates: CollectionUpdate[];
}

export async function checkForUpdates(): Promise<UpdateInfo> {
  return invoke("check_for_updates");
}

export interface CollectionInfo {
  id: string;
  display_name: string;
  torrent_file: string;
}

export async function getAvailableCollections(): Promise<CollectionInfo[]> {
  return invoke("get_available_collections");
}

export async function scanInstalledGames(): Promise<number> {
  return invoke("scan_installed_games");
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
