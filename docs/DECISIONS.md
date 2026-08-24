# Decisions

## 2026-08-14 - LP-Matching family-scoped, Bundled-DB v7
- Entscheidung: Jeder LP↔EN-Match (Shortcode, thumbnail_key, dosbox_conf, has_thumbnail) läuft über `same_group`/`family_expr`; `refresh_catalog` ruft die Verknüpfung nach dem Row-Copy selbst auf.
- Verworfen: thumbnail_key-Kopie in setup.rs behalten - zweite Kopie der Regel driftete bereits (unscoped), gelöscht zugunsten `propagate_lp_thumbnail_keys`.
- Grund: Titel UND Shortcodes wiederholen sich über Pack-Familien (GK2-DE bekam den eXoWin9x-Key, Cover 404te); der Katalog-Refresh stellte die kaputten Keys aus der Bundled-DB bei jedem Start wieder her.
- Gotcha: Die Bundled-DB stempelt IHRE eigene Version; solange sie älter ist als CATALOG_VERSION, läuft der Refresh bei jedem Start - Heilungen müssen also im Refresh selbst passieren, nicht nur in migrate().

## 2026-08-19 - Bibliothek folgt der Absicht, nicht der Platte
- Entscheidung: Der automatische Scan darf Bibliothekseintraege nur noch BESTAETIGEN; `adopt_from_disk` (Import, Rescan-Knopf, Datenverzeichniswechsel) ist der einzige Weg, aus einem Archiv einen Eintrag zu machen. Dazu `collateral_file_indices`: Dateien, deren saemtliche Pieces von angeforderten Dateien belegt sind, fliegen aus der Bibliothek.
- Verworfen: ZIP-Integritaetspruefung und Groessenschwelle als Erkennung - Kollateral ist vollstaendig und valide (8 MiB Piece, eXoDOS-Archive meist kleiner), also nicht am Inhalt erkennbar.
- Grund: Zwei Nutzerreports (4 -> 17 Spiele nach einem Download; vier ZZT-Titel nach util.zip). Pass 2 zaehlte jedes Archiv >= 1 KB als Installation.
- Gotcha: Kollateral-Bereinigung und Adoption sind Gegensaetze und laufen nie zusammen - sonst entfernt der Scan Zeilen, die er eine Zeile spaeter wieder anlegt. Die Sollgroesse kommt aus dem gebuendelten .torrent, nicht aus `download_size` (das ist ZIP + GameData).
