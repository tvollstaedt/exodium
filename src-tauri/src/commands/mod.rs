pub(crate) mod content_packs;
mod games;
pub(crate) mod setup;
pub(crate) mod updates;

pub use content_packs::{
    cancel_content_pack_install, get_content_pack_progress, install_content_pack,
    list_content_packs, uninstall_content_pack, ContentPackState,
};
pub use games::{
    cancel_download, collection_data_dir, download_game, get_config, get_download_progress, get_game,
    get_game_settings, get_game_variants, get_games, get_genres, get_recently_played,
    get_section_keys, get_installed_games, launch_game, set_config, set_game_settings,
    toggle_favorite, uninstall_game, DbState,
};
pub use setup::{
    bundled_metadata_dir, factory_reset, game_name_from_app_path,
    get_available_collections, get_default_data_dir, get_game_metadata, get_poster_dir,
    get_preview_dir, get_setup_status, get_thumbnail_dir, get_torrent_info,
    init_download_manager, init_resource_dir, scan_installed_games, setup_from_local,
    setup_import, setup_start, validate_exodos_dir, CollectionDef, COLLECTION_MAP,
    TorrentState,
};
pub use updates::check_for_updates;
