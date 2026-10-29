# Song search API

`GET /api/songs/search?q=caf%C3%A9&more=false` requires the existing bearer access token. Any authenticated account can search, including outside a lobby. Every response has `Cache-Control: no-store`.

Supply `q` exactly once: 1–200 Unicode scalar values after trimming. Comparison ignores case, diacritics and repeated whitespace. Words match title or artist/channel prefixes; punctuation separates words and FTS operators are literal text. Optional `more` accepts exactly `true` or `false` (default). Unknown/duplicate parameters, invalid encoding and request bodies are rejected. Query strings above 8 KiB return 414.

```json
{"results":[{"video_id":"dQw4w9WgXcQ","title":"Never Gonna Give You Up","artist":"Rick Astley","thumbnail_url":null,"watch_url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ","source":"catalog"}],"discovery":{"status":"missing_credentials"}}
```

Titles and attribution are decoded plain text; render them as text. Missing thumbnails are null. Results contain up to ten catalog matches, ranked by relevance, accepted submission count and video ID, followed by up to ten distinct external results in provider order. Source is `catalog`, `youtube` or `youtube_cache`. Counts, account identities, membership data and search histories are not returned.

| Discovery status | Meaning |
| --- | --- |
| `not_needed` | At least five catalog matches and `more=false`; no discovery work |
| `cached` | A valid saved search was reused, including an empty snapshot |
| `fresh` | Validated provider results obtained, including empty results; persistence failure does not discard them |
| `missing_credentials` | No saved search and no configured key |
| `daily_limit_reached` | No saved search and the application attempt limit is exhausted |
| `temporarily_unavailable` | Provider, cache or allowance access failed before usable discovery results were obtained |

Discovery runs only below five catalog matches or with `more=true`. Valid saved results take precedence over missing credentials and exhausted allowance. Equivalent case/accent/whitespace queries share an in-flight attempt and seven-day saved results. `more=true` does not bypass cache or request another page. Successful persistence is required for durable reuse; failed writes or corrupted cache data can permit a later guarded attempt. Cache storage outages suppress external calls. Fresh validated results survive write failure/timeouts, and initial catalog results remain available during external failures.

To enqueue a result, use its `watch_url` with the existing [queue API](queue-api.md):

```http
POST /api/lobbies/memberships/{membership_id}/queue/entries
Authorization: Bearer <access_token>
Content-Type: application/json

{"url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ","request_id":"b8c4c8af12484620b2e55ee13f45ab91"}
```

Reuse the same request ID and video for retries of one intended contribution. An intentional repeat uses a new ID. Active membership and authoritative queue metadata validation still apply. Search never enqueues anything. Newly accepted contributions from searched or pasted links attempt independent catalog recording and one popularity increment per occurrence; recording failures preserve queue success. Receipt replays do not increment popularity. A crash between queue commit and catalog recording can undercount.

Optional configuration in `backend/.env` for cargo, or root `.env` for Compose:

```dotenv
YOUTUBE_API_KEY=
YOUTUBE_SEARCH_DAILY_LIMIT=90
```

Absent/blank keys allow catalog-only operation and saved-result reuse. The key is independent of OAuth login configuration. The cap accepts nonnegative 64-bit integers, defaults to 90 and permits zero to disable fresh attempts; invalid values fail startup safely. Reservations persist across restarts and are never refunded after failure. The application's UTC-day reset differs from the provider quota reset; the cap does not guarantee provider availability. No live key is needed for automated tests.

Starter content is compiled from `backend/resources/starter-songs.json`, initially `[]`. Records contain `video_id`, `title`, `artist` and optional nullable `thumbnail_url`. Invalid records are diagnosed and skipped. Rebuild after edits; startup is idempotent and performs no provider requests. No runtime file mount is needed in Docker's `/data` directory.

Provider display metadata expires after 29 days; expired text is immediately excluded and removed at startup/periodic cleanup. Separate starter text remains available. Identities, popularity and contribution receipts survive metadata expiry and lobby deletion. Saved search snapshots expire independently after seven days; reads never extend either lifetime. Cleanup does not refresh the provider.

The supported topology is one backend process sharing one search service across requests. Persistent allowance is transactional across database connections; horizontal replicas require additional shared-flight coordination. Catalog reads have a one-second deadline; discovery has 4.5 seconds including a three-second provider request and at most 250 ms final persistence. Degraded responses target six seconds total.

See [error codes](api-errors.md) and [validation evidence](../specs/003-song-search/quickstart.md).
