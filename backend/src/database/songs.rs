//! Shared song catalog: every accepted submission plus a checked-in starter list,
//! searched with FTS5 over title and artist.
use super::Database;
use crate::{
    models::{queue::timestamp, song::SongResult, youtube::VideoMetadata},
    songs::{SongSearch, SongsError, StarterSong, decode_html_entities, thumbnail_url, watch_url},
};

impl Database {
    pub(super) async fn create_song_tables(&self) -> Result<(), sqlx::Error> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        for statement in [
            "CREATE TABLE IF NOT EXISTS songs (
                id INTEGER PRIMARY KEY NOT NULL,
                video_id TEXT NOT NULL UNIQUE CHECK(length(video_id) BETWEEN 6 AND 24 AND video_id NOT GLOB '*[^A-Za-z0-9_-]*'),
                title TEXT NOT NULL CHECK(length(title) > 0),
                artist TEXT NOT NULL,
                thumbnail_url TEXT NOT NULL,
                times_queued INTEGER NOT NULL DEFAULT 0 CHECK(times_queued >= 0),
                last_queued_at TEXT CHECK(last_queued_at IS NULL OR (length(last_queued_at) = 25 AND strftime('%Y-%m-%dT%H:%M:%S+00:00', last_queued_at) IS last_queued_at)),
                CHECK((times_queued = 0) = (last_queued_at IS NULL))
            ) STRICT",
            // External-content index over songs: `remove_diacritics 2` makes "beyonce" match
            // "Beyoncé"; prefix queries give partial-word matches.
            "CREATE VIRTUAL TABLE IF NOT EXISTS songs_fts USING fts5(
                title, artist, content = 'songs', content_rowid = 'id',
                tokenize = 'unicode61 remove_diacritics 2'
            )",
            "CREATE TRIGGER IF NOT EXISTS songs_fts_insert AFTER INSERT ON songs BEGIN
                INSERT INTO songs_fts(rowid, title, artist) VALUES (new.id, new.title, new.artist);
            END",
            "CREATE TRIGGER IF NOT EXISTS songs_fts_delete AFTER DELETE ON songs BEGIN
                INSERT INTO songs_fts(songs_fts, rowid, title, artist) VALUES ('delete', old.id, old.title, old.artist);
            END",
            "CREATE TRIGGER IF NOT EXISTS songs_fts_update AFTER UPDATE OF title, artist ON songs BEGIN
                INSERT INTO songs_fts(songs_fts, rowid, title, artist) VALUES ('delete', old.id, old.title, old.artist);
                INSERT INTO songs_fts(rowid, title, artist) VALUES (new.id, new.title, new.artist);
            END",
        ] {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        tx.commit().await
    }

    /// Insert or refresh the starter songs. Rerunning changes nothing; counts are never touched,
    /// and curated title/artist win over metadata recorded from earlier submissions.
    pub async fn load_starter_songs(&self, songs: &[StarterSong]) -> Result<u64, sqlx::Error> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut changed = 0;
        for song in songs {
            changed += sqlx::query(
                "INSERT INTO songs(video_id, title, artist, thumbnail_url) VALUES (?, ?, ?, ?)
                 ON CONFLICT(video_id) DO UPDATE SET title = excluded.title, artist = excluded.artist,
                    thumbnail_url = excluded.thumbnail_url
                 WHERE (title, artist, thumbnail_url) IS NOT (excluded.title, excluded.artist, excluded.thumbnail_url)",
            )
            .bind(&song.video_id)
            .bind(decode_html_entities(&song.title))
            .bind(decode_html_entities(&song.artist))
            .bind(thumbnail_url(&song.video_id))
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        tx.commit().await?;
        Ok(changed)
    }

    /// Count one accepted submission. Songs already in the catalog keep their title and
    /// artist; new ones take the server-resolved metadata.
    pub async fn record_queued_song(
        &self,
        video: &VideoMetadata,
        now: i64,
    ) -> Result<(), sqlx::Error> {
        let at =
            timestamp(now).map_err(|_| sqlx::Error::Protocol("timestamp out of range".into()))?;
        sqlx::query(
            "INSERT INTO songs(video_id, title, artist, thumbnail_url, times_queued, last_queued_at)
             VALUES (?, ?, ?, ?, 1, ?)
             ON CONFLICT(video_id) DO UPDATE SET times_queued = times_queued + 1, last_queued_at = excluded.last_queued_at",
        )
        .bind(&video.video_id)
        .bind(decode_html_entities(&video.title))
        .bind(decode_html_entities(&video.author_name))
        .bind(&video.thumbnail_url)
        .bind(at)
        .execute(&self.db_pool)
        .await?;
        Ok(())
    }

    /// Whole-word matches rank above prefix-only ones; within each, the most queued first,
    /// then FTS5 relevance (title weighted over artist).
    pub(crate) async fn search_songs(
        &self,
        user: i64,
        search: &SongSearch,
    ) -> Result<Vec<SongResult>, SongsError> {
        let mut tx = self.db_pool.begin().await?;
        let account: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ?)")
            .bind(user)
            .fetch_one(&mut *tx)
            .await?;
        if !account {
            return Err(SongsError::Unauthorized);
        }
        if search.is_empty() {
            return Ok(Vec::new());
        }
        let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
            "SELECT s.video_id, s.title, s.artist, s.thumbnail_url, s.times_queued
             FROM songs_fts JOIN songs s ON s.id = songs_fts.rowid
             WHERE songs_fts MATCH ?
             ORDER BY s.id NOT IN (SELECT rowid FROM songs_fts WHERE songs_fts MATCH ?),
                s.times_queued DESC, bm25(songs_fts, 2.0, 1.0), s.title
             LIMIT ?",
        )
        .bind(search.prefix_query())
        .bind(search.word_query())
        .bind(search.limit)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(
                |(video_id, title, artist, thumbnail_url, times_queued)| SongResult {
                    url: watch_url(&video_id),
                    video_id,
                    title,
                    artist,
                    thumbnail_url,
                    times_queued,
                },
            )
            .collect())
    }
}
