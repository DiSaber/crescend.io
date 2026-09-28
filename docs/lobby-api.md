# Lobby API: creation, joining, and recovery (US1–US3)

`POST /api/lobbies` creates a lobby and joins the authenticated account as its sole owner and first member in one transaction. The request body must be empty. Send the existing access token in the Authorization header; follow [browser authentication](browser-auth.md) to obtain it. The refresh cookie alone does not authorize this operation.

```js
const response = await fetch('/api/lobbies', {
  method: 'POST',
  headers: { Authorization: `Bearer ${accessToken}` },
});
const result = await response.json();
if (response.status === 201) {
  // Store result in the client's in-memory lobby state.
  // result.lobby.join_code is the shareable code.
}
```

Success is **201 Created**, `Content-Type: application/json`, and `Cache-Control: no-store`:

```json
{
  "membership_id": "22222222222222222222222222222222",
  "lobby": {
    "id": "11111111111111111111111111111111",
    "join_code": "A1B2C3",
    "owner_user_id": "42",
    "created_at": "2026-09-24T20:00:00Z",
    "expires_at": "2026-09-25T20:00:00Z",
    "revision": "1",
    "members": [
      { "user_id": "42", "role": "owner", "joined_at": "2026-09-24T20:00:00Z" }
    ]
  }
}
```

The lobby ID and caller's membership ID are independent 128-bit identifiers represented as 32 lowercase hex characters. The six-character invitation code uses uppercase letters and digits. Account IDs and revisions are decimal strings; timestamps are UTC RFC3339. Provider identifiers and credentials are never included. Ownership is derived from the authenticated account, not client input.

A user can belong to at most one active lobby. Creation while already in a lobby returns 409, including retries after a successful response was lost. A lobby expires exactly 24 hours after creation. An expired membership does not block creating a new lobby, even before periodic cleanup removes its rows. Disconnecting, signing out, or restarting the backend does not remove unexpired membership.

## Join by code

`POST /api/lobbies/join` accepts a JSON object containing only `join_code`, with a maximum body size of 1 KiB:

```js
const response = await fetch('/api/lobbies/join', {
  method: 'POST',
  headers: {
    Authorization: `Bearer ${accessToken}`,
    'Content-Type': 'application/json',
  },
  body: JSON.stringify({ join_code: '  a1b2c3  ' }),
});
const result = await response.json();
if (response.ok) {
  // Replace the in-memory lobby state with result.
}
```

The server trims surrounding ASCII whitespace and ignores ASCII letter case, then requires six letters/digits. Success returns **200 OK** with the same CurrentLobby shape shown above, including the caller's membership ID and the complete roster. A new join adds one `member`, increments revision once, and preserves the original owner and expiration. Membership grants no administration permissions; ownership cannot be requested or transferred.

Retrying the same lobby returns the existing membership generation and current roster without changing revision. This also applies to the creator. Joining another active lobby returns 409 without moving the caller. Unknown, closed, and expired codes all return the same 404 `lobby_unavailable` response without roster information. A stale membership from a closed or expired lobby does not block joining an active lobby.

Malformed JSON, missing fields, duplicate fields, wrong field types, and unknown fields return 400 `invalid_request`. A malformed normalized code returns 400 `invalid_join_code`. Bodies over 1 KiB return 413 `request_too_large`; missing or non-JSON Content-Type returns 415 `unsupported_media_type`. All join responses carry `Cache-Control: no-store`.

## Recover current state

Available after the US3 checkpoint, `GET /api/lobbies/current` returns **200 OK** with the authenticated account's current lobby, or `{ "membership_id": null, "lobby": null }`. Both fields are null together when membership is absent, closed, or expired, even before database cleanup. All responses carry `Cache-Control: no-store`.

```js
const response = await fetch('/api/lobbies/current', {
  headers: { Authorization: `Bearer ${accessToken}` },
  cache: 'no-store',
});
if (!response.ok) {
  // Handle 401 through the existing browser authentication coordinator.
  // A storage failure is not an empty membership result.
  throw new Error(`Current lobby request failed: ${response.status}`);
}
const current = await response.json();
if (current.membership_id === null && current.lobby === null) {
  // Clear the client's in-memory lobby state.
} else {
  // Replace it with current, including the caller's membership generation.
}
```

Use this read after reload, sign-in on another client, or a lost creation/join response. Clients for the same account recover the same generation; different members see the same roster but only their own membership ID. The server selects identity from the verified bearer token; no user-ID or lobby-ID selector is supported. Members are sorted by join time, then numeric user ID, with roles derived from the original owner. Each response is one coherent snapshot, including its revision. A concurrent join may appear in this read or the next. Use the US4 stream below to invalidate and refetch this snapshot.

## Automatic updates (US4)

Open `GET /api/lobbies/memberships/{membership_id}/events` with the caller's generation and bearer token. Use fetch streaming, which supports the Authorization header. Keep credentials in memory and out of URLs; the refresh cookie cannot authorize this route. Native EventSource is not suitable for this authentication contract.

The server checks account, generation ownership and lobby activity before returning 200 `text/event-stream`. Malformed IDs return 400 `invalid_request`; absent, ended and foreign generations return identical 403 `membership_forbidden` errors. Authentication failures return 401. All responses are `Cache-Control: no-store`; successful streams also send `X-Accel-Buffering: no`.

| Event | Data | Action |
| --- | --- | --- |
| `sync_required` | `{"revision":"1"}` | Refetch current state on every connection |
| `lobby_changed` | `{"revision":"2"}` | Coalesce hints and refetch current state |
| `membership_ended` | `{"reason":"expired"}` | Clear this generation and close |
| `auth_expired` | `{}` | Close and recover authentication through the existing coordinator |

Events contain no roster, invitation code, or other account's generation. Comments keep an authorized connection alive every 15 seconds. Expiration ends idle streams independently of database cleanup. Disconnecting does not change membership. One-second reconciliation recovers committed changes even if their post-commit notification was lost. Notifications are not a durable event log; `Last-Event-ID` does not replay history.

This transport example parses UTF-8 and SSE framing across arbitrary chunks, including CRLF boundaries and multiple events in a chunk. `onEvent` should enqueue invalidations synchronously so it does not block reading the stream while fetching state. It is a client contract example, not a frontend application.

```js
async function consumeLobbyEvents(membershipId, accessToken, signal, onEvent) {
  const response = await fetch(
    `/api/lobbies/memberships/${encodeURIComponent(membershipId)}/events`,
    { headers: { Authorization: `Bearer ${accessToken}`, Accept: 'text/event-stream' },
      cache: 'no-store', signal },
  );
  if (!response.ok) throw new Error(`Stream HTTP ${response.status}`);
  const reader = response.body.getReader();
  const decoder = new TextDecoder('utf-8', { fatal: true });
  let buffer = '';
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break; // Reconnect and synchronize after an unexpected EOF.
      buffer += decoder.decode(value, { stream: true });
      if (buffer.length > 65536) throw new Error('Oversized event buffer');
      for (;;) {
        const boundary = /\r?\n\r?\n/.exec(buffer);
        if (!boundary) break;
        const block = buffer.slice(0, boundary.index);
        buffer = buffer.slice(boundary.index + boundary[0].length);
        let event = 'message';
        const data = [];
        for (const line of block.split(/\r?\n/)) {
          if (line.startsWith('event:')) event = line.slice(6).replace(/^ /, '');
          if (line.startsWith('data:')) data.push(line.slice(5).replace(/^ /, ''));
        }
        if (data.length) onEvent(event, JSON.parse(data.join('\n')));
      }
    }
  } finally {
    await reader.cancel();
    reader.releaseLock();
  }
}
```

Serialize current-state requests: maintain an in-flight flag and a dirty flag. A hint sets dirty; start a read only when none is in flight, clearing dirty as it starts. If another hint arrives during that read, dirty remains set and triggers another read after completion. Duplicate hints at or below the applied/pending revision can be coalesced; every `sync_required` forces a read. Compare decimal revisions with `BigInt`, not lexicographically or with floating-point numbers, and never apply a lower revision within the same lobby/generation.

Associate streams and reads with a local generation/connection epoch. Discard callbacks and completed responses from a locally ended or superseded epoch. Paired null current state clears the view and closes its stream. If current state discovers a new generation, replace the old connection and reset revision tracking. Never apply a delayed terminal event from the previous connection to the new membership. After an unexpected EOF or storage/network failure, retain uncertainty and refetch; it is not evidence of departure.

Reconnect transient failures after an initial random 100–500 ms delay, then exponentially back off with jitter to a maximum of 10 seconds. Reset backoff after healthy synchronization. On 403, refetch current state to discover an ended or replacement generation. On 401 or `auth_expired`, use the same serialized refresh/logout coordinator described in [browser authentication](browser-auth.md); never blindly retry an uncertain refresh response. Replace tokens before planned expiration where possible, abort requests/streams on local logout, and ignore old callbacks. Sign-out retains persistent membership and existing access-token expiry semantics.

Disable buffering and caching in any reverse proxy for this route, forward `X-Accel-Buffering: no`, and set the proxy idle timeout above the 15-second keepalive interval. Verify comments and events arrive incrementally through the actual proxy using `curl --no-buffer` as shown in the feature quickstart. A slow client has no private application queue: the bounded wakeup channel may report lag, after which the stream reads authoritative state. Reconnection always synchronizes.

## Shared errors and creation input errors

| Status | Body / meaning |
| --- | --- |
| 400 | `{"error":"Invalid request body or membership ID.","code":"invalid_request"}` |
| 401 | `{"error":"Authentication failed.","code":"unauthorized"}`; missing/invalid/expired token or account no longer exists; `WWW-Authenticate: Bearer` |
| 409 | `{"error":"You already belong to an active lobby.","code":"already_in_lobby"}` |
| 500 | `{"error":"Lobby operation could not be completed.","code":"internal_error"}`; transaction fails without partial membership/lobby |
| 503 | `{"error":"Lobby operation is temporarily unavailable.","code":"temporarily_unavailable"}`; temporary database contention/pool exhaustion or ID/code allocation failure |

Errors also carry no-store. Internal database diagnostics do not appear in response bodies. Creation is not a way to switch lobbies. Do not expect a second lobby when retrying after an uncertain response.

## Available increment and development setup

Creation, joining, current-lobby retrieval, and membership event streams are implemented. Scalar advertises these four routes. Explicit leave and creator-triggered closure remain deferred to US5.

The creation response replaces the prototype's 200/bare-string response. This development change uses a fresh database, without migrations. Stop the backend before removing its old `backend/data.db` and SQLite sidecars (`data.db-wal`, `data.db-shm`), then restart and sign in again. The implementation workflow performs this authorized reset once; do not delete the database on routine restarts. The new schema retains `expires_at`. Existing authentication schemas and browser credential handling are unchanged.

Run from `backend/`:

```powershell
cargo fmt --check
cargo check --locked
cargo test --locked
cargo run --locked
```

Open `/scalar` on the backend origin to inspect the implemented contract. Automated creation tests use temporary file-backed databases and local test-only accounts/tokens; they do not require a real Google sign-in.
