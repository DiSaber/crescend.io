# Lobby API: creation (US1)

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

## Errors

| Status | Body / meaning |
| --- | --- |
| 400 | `{"error":"This operation requires an empty request body.","code":"invalid_request"}` |
| 401 | `{"error":"Authentication failed.","code":"unauthorized"}`; missing/invalid/expired token or account no longer exists; `WWW-Authenticate: Bearer` |
| 409 | `{"error":"You already belong to an active lobby.","code":"already_in_lobby"}` |
| 500 | `{"error":"Lobby creation could not be completed.","code":"internal_error"}`; transaction fails without partial membership/lobby |
| 503 | `{"error":"Lobby creation is temporarily unavailable.","code":"temporarily_unavailable"}`; temporary database contention/pool exhaustion or ID/code allocation failure |

Errors also carry no-store. Internal database diagnostics do not appear in response bodies. Creation is not a way to switch lobbies. Do not expect a second lobby when retrying after an uncertain response.

## Available increment and development setup

Only creation is implemented in US1. Join, current-lobby retrieval, SSE updates, leave, and creator-triggered closure remain planned; their endpoints are not registered or advertised in Scalar. Keep the creation response in client state for now. After losing it, there is no recovery endpoint in this increment; one is planned for US3.

The creation response replaces the prototype's 200/bare-string response. This development change uses a fresh database, without migrations. Stop the backend before removing its old `backend/data.db` and SQLite sidecars (`data.db-wal`, `data.db-shm`), then restart and sign in again. The implementation workflow performs this authorized reset once; do not delete the database on routine restarts. The new schema retains `expires_at`. Existing authentication schemas and browser credential handling are unchanged.

Run from `backend/`:

```powershell
cargo fmt --check
cargo check --locked
cargo test --locked
cargo run --locked
```

Open `/scalar` on the backend origin to inspect the implemented contract. Automated creation tests use temporary file-backed databases and local test-only accounts/tokens; they do not require a real Google sign-in.
