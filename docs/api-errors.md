# API errors

Authentication, lobby, and YouTube endpoint errors share an `application/json` body:

```json
{"code":"invalid_request","error":"Readable explanation."}
```

Clients should branch on the HTTP status and stable `code`, and use `error` for display. Authentication failures use `unauthorized`; existing lobby codes are unchanged. YouTube errors now return JSON, including body validation failures (400, 413, 415, 422). Routing-level 404/405 responses are outside this contract.

`backend/src/api_error.rs` owns `ErrorResponse` (the shared OpenAPI schema) and `ApiError` (status plus body serialization). Domain error enums retain their own variants and map to `ApiError` in their HTTP response implementations. Choose static, safe messages; log internal failure details separately. Endpoint-specific cookies, cache headers, and bearer challenges remain at their existing boundaries.

## Queue errors

Reordering returns `409 stale_queue_revision` for any mismatched lobby revision,
`409 entry_not_upcoming` for current, `404 queue_entry_unavailable` for an absent
active target and `400 invalid_request` for an invalid upcoming index. Authorization
precedes target/revision checks; stale revisions require authoritative recovery.

Voting requires guests (`403 guest_required`); policy changes require hosts
(`403 host_required`). An inactive or foreign vote target returns
`404 queue_entry_unavailable` after authorization. Invalid policy variants/counts
return `400 invalid_request`; fixed thresholds accept integers 1–2147483647.

Host advancement returns `409 current_entry_changed` when the expected occurrence
is no longer current, including an empty queue. Refetch before reevaluating intent.

Direct removal requires the host (`403 host_required`). Malformed entry IDs or
nonempty bodies return `400 invalid_request`; a valid absent ID is a 200 no-op.

Queue operations use the same JSON `{code,error}` envelope and bearer challenge.
`400 invalid_request` covers malformed IDs, JSON and extra/duplicate fields;
`413 request_too_large` covers bodies above 4 KiB; `415 unsupported_media_type`
requires JSON. Unsupported links return `422 invalid_url`. Metadata transport or
deadline failures return `502 upstream_unavailable`; invalid, oversized or missing
provider metadata returns `502 metadata_unavailable`. Receipt content conflicts
return `409 idempotency_conflict`. Ended/foreign generations return
`403 membership_forbidden`. Storage contention, pool failure, identity allocation
or checked revision exhaustion return `503 temporarily_unavailable`; unexpected
storage failures return sanitized `500 internal_error`. Failures preserve queue state.

## Song search errors

All song search responses, including authentication errors, use `Cache-Control: no-store`.
`400 invalid_search_request` covers missing/empty/overlong q, unknown or duplicate
parameters, invalid more/encoding and nonempty bodies. `414 search_uri_too_long`
means the raw query string exceeds 8 KiB. `401 unauthorized` uses existing bearer
verification before any catalog or allowance work. Catalog read failure/deadline
returns `503 temporarily_unavailable`; unexpected internal failures use the safe
`500 internal_error` envelope. Provider details, keys and internal SQL are never
returned. Missing credentials, exhausted application allowance and provider
failures instead return 200 with catalog results and a typed discovery status;
they do not produce 429 or 502. See [song search](song-search-api.md).

## Game errors

Game routes use the same envelope, bearer challenge and `Cache-Control: no-store`
as the queue. `400 invalid_request` covers malformed IDs, JSON, extra fields and an
advance body without exactly one of `phase_seq`/`play_seq`; `413 request_too_large`
and `415 unsupported_media_type` match the queue. Host-only routes return
`403 host_required`; ended or foreign generations `403 membership_forbidden`.
`400 invalid_game_settings` is a settings override outside the mode's served
schema (unknown key, wrong type, out of range or off the step grid);
`400 invalid_game_rules` a rule combination the engine cannot run.
`404 game_mode_unavailable` is an unknown mode; `404 game_submission_unavailable`
a vote target that is not on the current ballot. Conflicts (all 409): `game_running`
(start while running), `game_not_running`, `game_phase_closed` (action not open in
this phase), `game_phase_changed` (stale `phase_seq`), `game_song_changed`
(`play_seq` is not the playing song), `submission_limit_reached`,
`vote_limit_reached`, `self_vote_forbidden`, `idempotency_conflict`, and for Name
That Tune `song_already_proposed`, `own_song` (the proposer guessing their own song)
and `guess_limit_reached`. The host guessing gets `403 host_cannot_guess`.
Prompts that do not fit the mode return `400 invalid_game_prompts`; an image that is
not in the lobby, or not yet visible to the caller, `404 game_image_unavailable`.
Uploads: `413 image_too_large` (over 5 MB), `415 unsupported_image_type` (not JPEG,
PNG or WebP), `422 image_dimensions_exceeded` (over 4096 px), `422 invalid_image`
(unreadable), `409 image_limit_reached` (30 per lobby) and, for deletion,
`409 image_in_use`. Song links
and metadata fail like the queue (`422 invalid_url`, `502 upstream_unavailable`,
`502 metadata_unavailable`). Contention is `503 temporarily_unavailable`; unexpected
storage failures are a sanitized `500 internal_error`. Failures change nothing.
See [game API](game-api.md).
