# Guest join and nicknames

People join a lobby with the code and a nickname. They don't need a Google account.
Hosting still needs Google, so every lobby belongs to a real account.

## What users see

- **Join page** (`/join/`, and the QR link `/join/?code=XXXXXX`): a code box and a
  nickname box. Submitting signs the browser in as a guest (when it is not already
  signed in) and joins. There is no Google redirect. A returning browser sees its last
  nickname filled in. A signed-in Google user joins as themselves and also picks a
  nickname (their last one is filled in).
- **Create page**: still behind Google sign-in. A signed-in guest who opens it is
  told hosting needs Google and gets the Google button. The host picks a nickname too
  (their last one is filled in).
- **Names everywhere**: the people list, "added by" on queue songs, game results,
  now playing ("picked by"), the scoreboard, the TV board and the Name That Tune reveal
  show nicknames. The caller's own name is marked "(you)" and the host's has "· host".
  Ballots stay anonymous, as the game design requires (`docs/game-api.md`): no
  submitter names are shown until results.

## Guest accounts

`POST /api/auth/guest` (CSRF header, empty body, same rules as refresh) signs the
browser in as a guest. It returns the same `{access_token, token_type, expires_in}`
as refresh and sets the same rotating `crescend_refresh` cookie a Google login sets,
so every other endpoint, the auth coordinator, the SSE stream and expiry work
unchanged. It also sets `crescend_guest`: an HttpOnly, SameSite=Lax cookie on
`/api/auth`, Secure outside localhost, holding a random 32-byte guest key. It lasts
400 days (the browser maximum) and is renewed on every guest sign-in.

- The account is an ordinary `users` row. Its subject is `guest:` + SHA-256(guest
  key). The key itself is never stored, so a database leak cannot be replayed as a
  cookie.
- The next guest sign-in from the same browser finds that subject and reuses the
  account, including after logout or after the 30-day refresh session ends. Votes,
  game points and the last nickname are keyed by account, so they come back on
  rejoin. A browser that already holds a refresh session has that session revoked and
  replaced, not stacked.
- `GET /api/auth/guest` changes nothing. It returns
  `{"returning": bool, "last_nickname": string|null}` for the browser's guest cookie,
  so the join page can fill in the nickname before signing in.
- `GET /api/me` (bearer) returns `{"user_id", "guest", "last_nickname"}`. The UI uses
  it to send guests from Create to Google and to fill in the nickname.
- `POST /api/lobbies` returns **403 `google_account_required`** for a guest account.
  Joining, queueing, voting, song search and games work the same for guests and Google
  users.

### Why a namespaced subject instead of a nullable `google_sub`

`users` is a STRICT table with `google_sub TEXT NOT NULL UNIQUE`. SQLite cannot drop
NOT NULL in place. A nullable column would mean rebuilding `users`, and with it every
foreign key that points at it (refresh sessions, lobbies, memberships, queue, votes,
games). Google subjects are digit strings, so a `guest:` prefix can never collide with
one. It also keeps the uniqueness that refresh and lookup rely on, and needs no
migration: existing rows stay as they are. `User::is_guest()` is the single place that
reads the marker.

## Nicknames

- **Rules**: trimmed, with inner whitespace runs collapsed to one space, then 1–24
  characters (Unicode scalar values). Control characters (Cc) are rejected, plus
  zero-width and bidirectional formatting characters (U+200B–U+200F, U+202A–U+202E,
  U+2060–U+206F, U+FEFF and a few others). Those can make two names look alike or
  reorder the text around them. Anything else, including `<`, `>` and quotes, is plain
  text. The backend never builds HTML. The frontend renders names only through JSX
  text, which escapes them (there is no `dangerouslySetInnerHTML` in the app). A
  violation returns **400 `invalid_nickname`**.
- **Unique per lobby**, ignoring case: a clash gets " 2", " 3", … appended, with the
  name shortened so the result still fits 24 characters (`Sam`, `sam 2`, `SAM 3`).
- **Stored per lobby and account** in `lobby_nicknames(lobby_id, user_id, nickname,
  folded)` with `UNIQUE(lobby_id, folded)`, so one person can use different names in
  different lobbies. Rows outlive the membership and are deleted with the lobby. A
  departed player keeps their name on the scoreboard and in "added by", the name stays
  reserved for them, and their rejoin gets it back instead of "Kat 2".
- **Last nickname**: `user_nicknames(user_id, nickname)` keeps the last name the
  account chose (as typed, before any suffix). It is offered on the join and create
  pages.
- **API**: `POST /api/lobbies/join` takes an optional `nickname`. Without one, the
  member keeps their name in that lobby, or gets their last nickname, or "Player".
  Sending a different nickname to the lobby you are already in renames you; this
  increments the revision and notifies like any other roster change. `POST
  /api/lobbies` takes an empty body (last nickname, or "Host") or `{"nickname": ...}`.
  Responses carry `members[].nickname`, `submitted_by_nickname` on queue entries and
  results, `now_playing.submitted_by_nickname` (null while hidden, like the user ID),
  `scoreboard[].nickname` and `last_result.guesses[].nickname`. Game results are
  stored with user IDs and named on every read, so they always show current names.

## Abuse limits

- **New guest accounts per client address**: at most 20 per sliding hour, kept in
  memory (`auth::guest::GuestLimiter`, forgotten on restart, which is fine for a
  nuisance limit). The 21st gets **429 `too_many_guests`** with `Retry-After`. Only
  account creation counts. A browser restoring its guest account, refresh and Google
  login are not limited. IPv6 clients share one bucket per /64, because one household
  usually owns a whole /64.
- **Which address**: the backend reads `X-Crescend-Client-IP` and falls back to the
  TCP peer when the header is absent (tests, a backend run without Caddy). Caddy sets
  that header on every proxied request with `header_up X-Crescend-Client-IP
  {client_ip}`, overwriting any client-sent copy. `{client_ip}` is Caddy's resolved
  client address:
  - Hosted demo: Cloudflare edge → `cloudflared` → Caddy on the loopback-published
    port → backend. Caddy sees the Docker bridge gateway (a private address). The
    Caddyfile trusts only private peers (`trusted_proxies static private_ranges`) and
    reads `client_ip_headers CF-Connecting-IP X-Forwarded-For`. Cloudflare sets
    `CF-Connecting-IP` to the visitor's address and overwrites any copy the visitor
    sends, so that is the header that matters. `X-Forwarded-For` is only a fallback
    for a private proxy that doesn't send it.
  - Public 80/443 (`docker-compose.hosting.yml`): internet peers are not trusted, so a
    spoofed `CF-Connecting-IP` is ignored and the TCP address is used. Docker's default
    published-port NAT keeps the client's source address.
  - The backend publishes no port (`expose` only), so the header can only come from
    Caddy.
- **Song search**: guests can search. The existing daily search cap protects the
  YouTube API quota whoever searches.
- **Known gap: two browsers, two votes.** Someone can open a second browser or a
  private window, get a second guest account, and vote twice. The address limit only
  slows mass creation. The host's veto is the remedy for now. Tying guests to devices
  or requiring sign-in for votes would be the next step if this becomes a problem.

## Unchanged

Google login, refresh rotation, logout, the one-hour access tokens, the 30-day session
cap, lobby expiry, and moving a guest out of an old lobby when they join a new one
(#50/#53). Hosts still can't join another lobby without leaving theirs.
