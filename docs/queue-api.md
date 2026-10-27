# Shared queue API

Authenticated active members read `GET /api/lobbies/memberships/{membership_id}/queue`
and submit with `POST /api/lobbies/memberships/{membership_id}/queue/entries`.
Send a bearer access token; the membership generation must belong to that account.
Responses, including errors, use `Cache-Control: no-store`.

Submit JSON `{ "url": "https://youtu.be/dQw4w9WgXcQ", "request_id": "44444444444444444444444444444444" }`.
Bodies are limited to 4 KiB; unknown and duplicate fields are rejected. The server
resolves metadata, rechecks authorization, and appends inside a serialized transaction.
The first entry becomes current; subsequent entries become upcoming in committed order.
Video identity can repeat; each occurrence has a distinct entry ID.

Retain a request ID across uncertain retries; use a new ID for a deliberate repeat.
Matching retries return the original entry ID without another metadata request.
Reusing an ID for a different canonical video returns `409 idempotency_conflict`.
Receipts remain until closure or expiry, including after consumption. A retry never
resurrects an entry. After leaving/rejoining, use the new membership generation.

GET returns membership/lobby IDs, decimal-string revision, nullable current, ordered
upcoming and voting policy. Each entry contains server metadata, contributor account
ID as a decimal string, UTC submission time, active vote count and own `has_voted`.
Voter identities are private. Default policy is majority: strict majority of active
guests, excluding the host; zero guests exposes a null effective threshold.

Submission returns `{ "changed": true, "entry_id": "...", "active": true, "queue": { ... } }`.
Matching retries return `changed: false`. Snapshot fields describe one committed
revision. Rejected/no-op operations never increment revisions or publish hints.

Queue commits reuse the membership SSE stream. `sync_required` always forces a
membership and queue read; `lobby_changed` invalidates both. Compare revisions
numerically with BigInt, never lexicographically or as floating-point values.
Serialize refetches using in-flight/dirty flags; a hint during a read requires
another read. Ignore lower revisions and responses from obsolete connection or
membership epochs, including late mutation responses. Membership and queue GETs
can have different revisions; refetch the older view if a combined view requires
matching versions. No event replay is available. On uncertain network/storage
outcomes reconnect with existing backoff and force an authoritative read. A queue
403 requires membership recovery. Terminal events clear only their bound generation.
Keepalive comments remain 15 seconds; storage reconciliation runs every second,
recovering missed process-local publications and commits from other writers.

Hosts veto with bodyless `DELETE .../queue/entries/{entry_id}`. Guests cannot
directly remove songs, including their own submissions. A veto removes exactly
that occurrence, promotes the first surviving upcoming entry if current was
removed, and preserves survivor order. Missing/inactive/foreign IDs are identical
successful no-ops after host authorization. Mutation responses contain `changed`
and a coherent `queue`; receipts survive removal and retries never resurrect songs.

Host progression uses `POST .../queue/advance` with `expected_entry_id` and `reason`
(`ended`, `skip`, or `playback_error_skip`). All three explicitly consume only the
expected current occurrence. Empty queues or mismatches return `409 current_entry_changed`.
A delayed callback cannot consume its replacement even when both reference the same
video. On an uncertain response, refetch; never blindly substitute the new current ID.
A playback error alone does not advance state, and an empty queue does not close a lobby.

Guests cast a bodyless `POST .../queue/entries/{entry_id}/votes`. Only active guests
vote; hosts receive `403 guest_required`. Each account contributes at most one vote
per occurrence. Duplicate votes are no-ops while the target remains active; retired
targets return `404 queue_entry_unavailable`. Counts include only currently active
guest accounts. A departing account's retained vote stops counting, and rejoining
restores that same vote without permitting another. Responses expose only own
`has_voted`, never a voter roster.

Hosts `PUT .../queue/voting-policy` with `{"mode":"majority"}` or
`{"mode":"fixed","count":2}`. Fixed count is an integer from 1 through 2147483647;
majority forbids count. Extra/duplicate fields, null and fractional counts are invalid.
Identical policy updates are no-ops. Fixed thresholds remain configured above the
guest population. Zero guests never remove songs. Votes, policy changes and actual
guest joins/leaves reevaluate all entries atomically; every qualifying occurrence
is removed together, preserving survivor order and using one revision for the
membership or moderation change plus all removals.

Upgrade all backend membership writers together: old writers cannot perform vote
reevaluation. Queue order rewrites are linear in active queue length. Physical host
playback-device election remains separate from guarded occurrence progression.

Host reordering uses `POST .../queue/entries/{entry_id}/move` with
`{"expected_revision":"9","to_index":0}`. The index is zero-based within upcoming
after the move; current cannot move. Only the selected occurrence moves, preserving
other relative order, metadata and votes. Authorization precedes the revision check;
the revision check precedes lookup/index interpretation. Stale intent returns
`409 stale_queue_revision`; targeting current returns `409 entry_not_upcoming`.
Matching-position moves require a current revision and succeed unchanged. Recover
a lost move response by GET and reassessing intent. Buttons, drag or touch controls
can all use this same backend operation; player/frontend wiring is separate work.
