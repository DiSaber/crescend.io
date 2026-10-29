# Song catalog API

Authenticated accounts search with `GET /api/songs/search?q=crab%20ra&limit=10`.
Send a bearer access token; anonymous calls and tokens for unknown accounts return
`401 unauthorized` with `WWW-Authenticate: Bearer`. No lobby membership is required:
the catalog is shared by all lobbies. Responses, including errors, use
`Cache-Control: no-store`.

`q` is 1-100 characters after trimming; `limit` is an integer from 1 through 25 and
defaults to 10. Anything else returns `400 invalid_search`. The text is split on every
character that is not a letter or digit, and at most 8 words are used, so FTS5 syntax
in user input (`OR`, `NEAR(`, quotes, `*`) is matched as plain words. Text with no
letters or digits returns an empty result list.

Each word matches as a word prefix in the title or artist, case-insensitively and
ignoring accents: `cra ra` finds "Crab Rave", `beyonce` finds "Beyoncé", `senorita`
finds "Señorita". All words must match. Ranking: songs where every word matches a whole
word come before prefix-only matches; within each group, the most queued first, then
FTS5 relevance with the title weighted twice the artist, then title.

Returns `{ "results": [ ... ], "youtube": "not_configured" }`. Each result has
`video_id`, `title`, `artist`, `thumbnail_url`, `times_queued` and `url`, a watch URL to
submit through `POST .../queue/entries` exactly like a pasted link (same validation,
metadata lookup and retry rules; see queue-api.md). `youtube` reports whether YouTube
was searched; only `not_configured` exists until the YouTube Data API fallback (#41
part 2) adds its outcomes. Search never calls YouTube today.

## Storage

`songs` is a STRICT table: `video_id` (unique), `title`, `artist`, `thumbnail_url`,
`times_queued` (0 or more) and `last_queued_at` (UTC, null until first queued).
`songs_fts` is an external-content FTS5 index over title and artist using
`unicode61 remove_diacritics 2`, kept in sync by insert/delete/update triggers. Both
are created in the startup create-tables pass.

Every accepted queue submission (`changed: true`) upserts its video and increments
`times_queued`; matching retries do not count again. A song already in the catalog
keeps its title and artist; a new one takes the server-resolved oEmbed title and
channel. Recording runs after the queue commit and is best effort: a failure is logged
and never fails or delays the submission's outcome. HTML entities (`&amp;`, `&#39;`,
`&quot;`, numeric) in titles and artists are decoded once before storing.

## Starter list

`backend/data/starter-songs.json` (video ID, title, artist) is compiled into the binary
and loaded at every startup. Loading is idempotent: new songs are inserted, changed
titles/artists/thumbnails are updated, and counts are never touched. Songs removed from
the file stay in the catalog.

The list has 95 songs: the `demo-songs.txt` picks, recent hits and party staples, each
an upload from the artist's official, VEVO, "Topic" or label channel. Each was checked
on 2026-10-29 with YouTube oEmbed, which confirms the upload and channel and returns
401 when embedding is disabled. That does not prove playback on every origin; a video
that still refuses to play is skipped by the host's `playback_error_skip`.
