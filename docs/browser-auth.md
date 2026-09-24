# Browser authentication

The frontend and API must share one origin (scheme, host, and port). Serve frontend pages and proxy `/api` through that origin, including in development. A frontend dev server on another port must use a same-origin proxy; do not call the backend directly across origins. The backend and proxy must not enable cross-origin credentialed CORS for authentication endpoints.

Keep the existing `GOOGLE_REDIRECT_URI` and its Google Cloud Console registration unchanged. It remains the backend callback URL, such as `http://localhost:3000/api/auth/google/callback`. Arrange the development proxy/frontend around that public origin. The callback origin also defines the trusted Origin for refresh and logout. There is no frontend-origin environment variable, and request Host/Forwarded headers do not define the trusted origin. Production requires HTTPS; the existing explicit localhost HTTP configuration disables Secure only for local development.

## Login and bootstrap

Navigate the top-level browser to the start endpoint:

```js
const destination = '/lobbies/create?from=login#details';
window.location.assign('/api/auth/google/start?' +
  new URLSearchParams({ return_to: destination }));
```

`return_to` is optional and defaults to `/` only when absent. It must be one nonempty root-relative destination, at most 2048 decoded bytes, optionally with query and fragment. Absolute URLs, `//` destinations, duplicates, malformed encoding, backslashes, controls, whitespace, unsafe path escapes, and paths that could escape the origin after normalization receive 400 before a login attempt is created. The normalized destination is stored with the browser-bound attempt; callback query parameters cannot replace it. It never changes Google's registered redirect URI.

On success, the callback commits a session, sets `crescend_refresh`, clears the temporary login cookie, and returns **303** to the stored page with an empty body. Credentials never appear in the redirect URL or callback JSON. The cookie is host-only, HttpOnly, Secure (except localhost HTTP), SameSite=Lax, and scoped to `/api/auth`.

After the page loads, obtain an access JWT with an empty-body POST:

```js
// Illustrative calls, to be placed inside the client's shared auth coordinator.
let accessToken; // Memory only: never localStorage, sessionStorage, or a URL.

async function bootstrap() {
  return navigator.locks.request('crescend-auth', async () => {
    // The coordinator must cancel queued calls if logout has begun in any tab.
    const response = await fetch('/api/auth/refresh', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'X-CSRF-Protection': '1' },
      // No body and no JSON refresh credential.
    });
    if (response.status === 401) {
      accessToken = undefined;
      throw new Error('Sign in again');
    }
    if (!response.ok) throw new Error(`Refresh failed: ${response.status}`);
    const token = await response.json();
    accessToken = token.access_token;
    return token;
  });
}
```

The response contains only `{access_token, token_type: "Bearer", expires_in: 3600}`. The browser receives the replacement refresh cookie separately; JavaScript must not read or store it. Bootstrap again after a page reload or when access expires. Send the access JWT for protected operations:

```js
const response = await fetch('/api/lobbies', {
  method: 'POST',
  headers: { Authorization: `Bearer ${accessToken}` },
});
```

The refresh cookie does not authorize lobby creation. Refresh and logout do not require a bearer token or contact Google. HttpOnly hides the refresh credential from scripts, but does not stop same-origin XSS from making requests.

## Serialize refresh and logout

Use one origin-wide queue for both operations, across requests and tabs. An origin-wide Web Lock (as above) can serialize network calls; coordinate in-memory access-token delivery and logout events through BroadcastChannel. A waiting tab may refresh sequentially using the browser's updated cookie. Do not cache a refresh credential or start parallel refreshes. Consumed-token reuse revokes the entire session; there is no retry grace period.

The snippets illustrate network calls, not a complete frontend coordinator. Before scheduling logout, the coordinator must broadcast logout intent, stop new refreshes, cancel queued refreshes in every tab, and clear each tab's in-memory access token. Check that cancellation state again after acquiring the lock. Allow any already running refresh to finish before logout takes the same lock. Keep refresh stopped until a new login is deliberately started.

```js
// Run after the coordinator has stopped/cancelled refreshes and notified all tabs.
async function logout() {
  accessToken = undefined;
  return navigator.locks.request('crescend-auth', async () => {
    const response = await fetch('/api/auth/logout', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'X-CSRF-Protection': '1' },
    });
    if (response.status !== 204) {
      throw new Error(`Logout failed: ${response.status}`);
    }
  });
}
```

Logout revokes only the cookie's session, including when it identifies an already consumed token. It clears the cookie after committed revocation. Missing, unknown, expired, and already revoked credentials produce idempotent 204. It does not delete the account, revoke other devices, or invalidate issued access JWTs: those remain usable until their original one-hour expiry. Dropping the in-memory JWT is still required on the frontend.

## Errors and recovery

Refresh and logout require exactly one `X-CSRF-Protection: 1`. When present, Origin must match the configured callback's normalized origin, and Sec-Fetch-Site must be `same-origin` (not `same-site`). Non-browser clients may omit those two metadata headers, but must still send the custom header and cookie. They must maintain a cookie jar.

| Response | Meaning and cookie behavior |
| --- | --- |
| 400 | Nonempty body, including legacy JSON; cookie unchanged. Fix the request. |
| 403 | Missing, wrong, or duplicate CSRF header, or unacceptable/duplicate origin metadata; no mutation or cookie changes. Fix client/proxy configuration. |
| 401 refresh | Missing, empty, ambiguous, invalid, expired, revoked, or reused cookie; cookie cleared. Clear frontend authentication state and start a fresh login. |
| 401 logout | Duplicate refresh cookies; cookie cleared. Treat authentication state as ended locally. |
| 500 | Storage or issuance failure; cookie unchanged. Refresh rotation rolls back. Logout has not reported success and can be retried. |

Do not automatically retry an uncertain or lost refresh response: the server may already have consumed the credential even if the replacement cookie never arrived. Stop the auth queue and recover with a new login instead of replaying a potentially consumed credential. On any 401, clear frontend auth state and require login before future refreshes. Logout is idempotent and may be retried after a lost response or a database failure; do not claim server logout succeeded until a 204 arrives.

Google callback failures remain generic JSON: 400 for invalid/expired/mismatched/replayed attempts, 401 for denial or invalid Google credentials, 503 for provider infrastructure failures, and 500 for persistence/issuance failures. Start a new login after a failed callback. Consumed attempts cannot be reused, their login cookie is cleared, and no new refresh credential is issued. A frontend error page is outside this change. Callback responses have `Cache-Control: no-store` and `Referrer-Policy: no-referrer`; every refresh/logout response is also no-store.

## Fixed session expiry and migration

A session expires **30 days after its original creation**. Rotation never extends that deadline. Cookie Expires equals the original deadline; Max-Age uses only its remaining lifetime. If expiry passes before the response is constructed, refresh fails with 401 and clears the cookie. Database expiry is authoritative. Expired sessions and their consumed-token history are periodically cleaned up; accounts retain their identities.

This is a breaking change for JSON-token clients. Sign in again; stop reading callback token JSON and stop sending `{refresh_token: ...}` to refresh. Use the browser/cookie-jar flow with the required header, empty requests, and access-only JSON. There is no legacy JSON fallback. Deploy a compatible client and same-origin proxy with this backend. A rollback requires a compatible backend/client pair and fresh login, not deleting users or resetting IDs.

The API implementation and these examples do not add frontend UI or client modules. Before syncing or archiving the `browser-auth-handoff` OpenSpec delta, sync the completed `add-google-auth` baseline first; its `user-auth` specification is the prerequisite. Neither change is archived by this implementation.
