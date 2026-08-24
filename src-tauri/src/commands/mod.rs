pub mod media;
pub(crate) mod content_packs;
mod games;
mod playlists;
pub(crate) mod setup;
pub(crate) mod updates;
pub(crate) mod win9x;

pub use content_packs::{
    cancel_content_pack_install, get_content_pack_progress, install_content_pack,
    list_content_packs, uninstall_content_pack, ContentPackState,
};
pub use games::{
    cancel_download, collection_data_dir, download_game, game_printing_unavailable, game_engine_info, get_config, get_download_progress, get_game,
    get_game_settings, get_game_variants, get_games, get_genres, get_recently_played,
    get_section_keys, get_installed_games, launch_game, open_manual, reset_game_data, set_config,
    set_game_settings,
    get_transfer_stats, set_rate_limits, set_seeding_enabled, toggle_favorite, uninstall_game,
    update_check_supported, DbState,
};
pub use playlists::{
    create_playlist, delete_playlist, get_game_playlists, get_playlists, rename_playlist,
    set_playlist_membership,
};
pub use setup::{
    bundled_metadata_dir, collection_base_id, factory_reset, game_name_from_app_path,
    get_available_collections, get_default_data_dir, get_game_metadata, get_log_dir,
    data_dir_is_empty, get_poster_dir, get_preview_dir, get_setup_status, get_thumbnail_dir, get_torrent_info,
    init_download_manager, init_log_dir, init_resource_dir, open_log_folder,
    scan_installed_games, setup_from_local, setup_import, setup_start, torrent_search_names,
    validate_exodos_dir,
    CollectionDef, COLLECTION_MAP, TorrentState,
};
