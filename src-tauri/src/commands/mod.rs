mod games;
pub(crate) mod setup;
mod updates;

pub use games::{
    cancel_download, collection_data_dir, download_game, get_config, get_download_progress, get_game,
    get_game_variants, get_games, get_genres, get_section_keys, get_installed_games, import_games, launch_game,
    set_config, toggle_favorite, uninstall_game, DbState,
};
pub use setup::{
    bundled_metadata_dir, factory_reset, game_name_from_app_path,
    get_available_collections, get_default_data_dir, get_setup_status, get_thumbnail_dir,
    get_torrent_info, init_download_manager, setup_from_local, setup_import, setup_start,
    validate_exodos_dir, CollectionDef, COLLECTION_MAP, TorrentState,
};
pub use updates::check_for_updates;
