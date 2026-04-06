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
  game_torrent_index: number | null;
  gamedata_torrent_index: number | null;
  download_size: number | null;
  has_thumbnail: boolean;
  dosbox_variant: string | null;
}

export interface GameList {
  games: Game[];
  total: number;
}

export async function getGames(
  page?: number,
  perPage?: number,
  query?: string,
  language?: string,
  genre?: string,
  sortBy?: string,
  collection?: string
): Promise<GameList> {
  return invoke("get_games", { page, perPage, query, language, genre, sortBy, collection });
}

export async function getGenres(): Promise<string[]> {
  return invoke("get_genres");
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

export async function getLanguages(): Promise<string[]> {
  return invoke("get_languages");
}

export async function getGame(id: number): Promise<Game | null> {
  return invoke("get_game", { id });
}

export async function importGames(zipPath: string): Promise<number> {
  return invoke("import_games", { zipPath });
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

export async function setupFromLocal(
  exodosPath: string,
  dataDir: string
): Promise<number> {
  return invoke("setup_from_local", { exodosPath, dataDir });
}

export async function initDownloadManager(): Promise<boolean> {
  return invoke("init_download_manager");
}

export async function factoryReset(): Promise<void> {
  return invoke("factory_reset");
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
