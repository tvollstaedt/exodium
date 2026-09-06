pub mod media;
pub(crate) mod collections;
pub(crate) mod paths;
pub(crate) mod assets;
pub(crate) mod layout;
pub(crate) mod library;
pub(crate) mod install;
pub(crate) mod content_packs;
mod games;
mod playlists;
pub(crate) mod setup;
pub(crate) mod updates;
pub(crate) mod scummvm;
pub(crate) mod win9x;

pub use content_packs::{
    cancel_content_pack_install, get_content_pack_progress, install_content_pack,
    list_content_packs, uninstall_content_pack, ContentPackState,
};
pub use games::{
    game_printing_unavailable, game_engine_info, get_config, get_game,
    get_game_settings, get_game_variants, get_games, get_genres, get_recently_played,
    get_section_keys, get_installed_games, launch_game, open_manual, set_config,
    set_game_settings,
    get_transfer_stats, set_rate_limits, set_seeding_enabled, toggle_favorite,
    update_check_supported, DbState,
};
pub use playlists::{
    create_playlist, delete_playlist, get_game_playlists, get_playlists, rename_playlist,
    set_playlist_membership,
};
pub use setup::{
    factory_reset,
    get_available_collections, get_default_data_dir, get_log_dir,
    data_dir_is_empty, get_setup_status, get_torrent_info,
    init_download_manager, open_log_folder,
    setup_from_local, setup_import, setup_start,
    validate_exodos_dir, TorrentState,
};
pub use install::{cancel_download, download_game, get_download_progress};
pub use library::{game_name_from_app_path, reset_game_data, scan_installed_games, torrent_search_names, uninstall_game};
pub use assets::{get_game_metadata, get_poster_dir, get_preview_dir};
pub use paths::{bundled_metadata_dir, init_log_dir, init_resource_dir};
pub use collections::{collection_base_id, CollectionDef, COLLECTION_MAP};
