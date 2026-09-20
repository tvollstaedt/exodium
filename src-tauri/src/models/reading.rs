use serde::{Deserialize, Serialize};

/// A magazine series, a book category or the catalog shelf - the grouping eXo
/// files an issue under in `Genre` (§19).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Publication {
    pub id: i64,
    /// `magazine` | `book` | `catalog`
    pub kind: String,
    pub name: String,
    pub issue_count: i64,
    pub first_year: Option<i64>,
    pub last_year: Option<i64>,
    pub cover_key: Option<String>,
    /// `EN` | `DE` - eXo files a translated series under its own name (§19).
    pub language: String,
}

/// One issue: a PDF to read, or one of the 241 disk magazines to launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub id: i64,
    pub key: String,
    pub publication_id: i64,
    pub publication: String,
    pub kind: String,
    pub title: String,
    pub sort_title: Option<String>,
    pub year: Option<i64>,
    pub release_date: Option<String>,
    pub publisher: Option<String>,
    pub developer: Option<String>,
    pub notes: Option<String>,
    /// The Media Pack archive this lives in, e.g. `Content/DOSMagazines.zip`.
    pub zip_file: String,
    /// Entry inside that archive; null for a runnable issue.
    pub entry_path: Option<String>,
    /// `pdf` | `image` | null
    pub entry_kind: Option<String>,
    pub size_bytes: i64,
    pub cover_key: Option<String>,
    pub runnable: bool,
    pub launch_dir: Option<String>,
    pub issue_dir: Option<String>,
    pub launch_bat: Option<String>,
    pub command_line: Option<String>,
    /// eXo's run.bat placeholders for this issue, as a JSON object.
    pub substitutions: Option<String>,
    pub favorited: bool,
    pub installed: bool,
    pub last_page: Option<i64>,
    pub last_opened: Option<String>,
    /// The torrent holding `zip_file`: a media source id or a collection id.
    pub source: String,
    /// The STORED archive inside `zip_file` the entries live in, if any.
    pub inner_zip: Option<String>,
    pub language: String,
    /// Cover-CD videos beside the issue - counted, never fetched.
    pub extras_count: i64,
}

/// eXo's own "this game was covered in …" link: magazine, page and the PDF it
/// points at, keyed by the game's shortcode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    /// `Review` | `Story` | `Preview` | `Walkthrough` | `Cheats` | `Hints`
    pub kind: String,
    pub page: i64,
    pub issue_key: String,
    pub issue_title: String,
    pub publication: String,
    pub year: Option<i64>,
    /// The issue's language, so the panel can lead with the article that
    /// matches the selected variant.
    pub language: String,
}
