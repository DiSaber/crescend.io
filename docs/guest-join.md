# Guest join and nicknames

People join a lobby with the code and a nickname. They don't need a Google account.
Hosting still needs Google, so every lobby belongs to a real account.

## What users see

- **Join page** (`/join/`, and the QR link `/join/?code=XXXXXX`): a code box and a
  nickname box. Submitting signs the browser in as a guest (when it is not already
  signed in) and joins. There is no Google redirect. A returning browser sees its last
  nickname filled in, but only while the box is empty and untouched, so a late answer
  never mixes with what the user is typing.
  - **Code box**: spaces, dashes and any other non-letters drop out as you type, and
    letters are upper-cased (`abc 123` becomes `ABC123`; a pasted join link gives its
    code). A code that isn't six letters or numbers is caught before anything is sent.
    A wrong code shows "No active session has that code" and creates no guest account
    (the page passes the code to guest sign-in, which checks it first).
  - **The host of this session** (signed in with Google, opening their own join link,
    say in a second tab) sees "You're hosting this session. To play as a guest too,
    open this link in a private window or on another device.", a "Go to my lobby"
    button and the link with a Copy button. There is no nickname form. The backend
    never renames the host through join either (see below).
  - **Any other signed-in Google user** picks: "Join as <nickname> with Google", or
    "Join as a guest instead", which signs this browser out of Google (the normal
    logout) and then runs the guest flow. If they host another lobby, a confirm warns
    first that it keeps running but they can't control it from this browser until they
    sign in again.
  - **Name suffix notice**: when the name comes back with a suffix, the lobby page
    says so once: You're "Kat 2" because "Kat" is taken.
- **Lobby page**: Google users see "Signed in with Google · Sign out" next to the
  leave button. Signing out while hosting asks first (the session keeps running, but
  this browser loses its host controls and player). The host's leave button reads "End
  session for everyone" and asks for confirmation, with a stronger warning while a game
  is running. Guests see "Leave session". The host sees "Remove" on every other person
  in the people list (with a confirm), for "ghosts" who closed their browser without
  leaving; see [removing members](#removing-members-and-ghosts).
- **Not in a session**: a signed-out visitor on `/lobby/` or `/lobby/tv/` gets "Join
  with a code" as the main action and "Host with Google" second. After a session ends
  (host ended it, removed, expired) the page offers "Join another session", plus "Host
  a session" only to Google users.
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
- `POST /api/auth/guest?join_code=XXXXXX` first checks the code (normalized like
  join): 400 `invalid_join_code` when malformed, 404 `lobby_unavailable` when no
  active lobby has it, and in both cases no account or cookie is created. Without the
  parameter it behaves as before.
- `GET /api/auth/guest` changes nothing. It returns
  `{"returning": bool, "last_nickname": string|null, "session": bool}` for the
  browser's cookies, so the join page can fill in the nickname before signing in.
  `session` says whether a refresh cookie is present at all; the auth coordinator asks
  once before its first refresh and skips a refresh that could only 401 (which browsers
  log as a console error on a first visit).
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
  user-perceived characters (extended grapheme clusters), so a family emoji
  (👨‍👩‍👧‍👦, 7 code points) or a flag (🏳️‍🌈) counts as one. One character may use
  at most 12 code points (enough for a skin-toned family, a cap on stacked combining
  marks); the tables' CHECK allows 24 × 12 = 288. Control characters (Cc) are
  rejected, plus zero-width and bidirectional formatting characters (U+200B–U+200F,
  U+202A–U+202E, U+2060–U+206F, U+FEFF and a few others) and variation selectors.
  Those can make two names look alike or reorder the text around them. Two exceptions
  make emoji work: a zero-width joiner (U+200D) is allowed only between an emoji (or
  its selector, skin tone or tag end) and another emoji, and U+FE0E/U+FE0F only right
  after an emoji or a keycap base (`0-9 # *`). So `Kat❤️` passes, while a lone
  zero-width space, `Kat` + ZWJ or `Kat` + U+FE0F fails. The frontend applies the same
  rules (`nicknameProblem`, using `Intl.Segmenter`). Anything else, including `<`,
  `>` and quotes, is plain text. The backend never builds HTML. The frontend renders names only through JSX
  text, which escapes them (there is no `dangerouslySetInnerHTML` in the app). A
  violation returns **400 `invalid_nickname`**.
- **Unique per lobby**, ignoring case: a clash gets " 2", " 3", … appended, with the
  name shortened by whole characters (never splitting an emoji) so the result still
  fits 24 (`Sam`, `sam 2`, `SAM 3`).
- **Older databases**: tables created when the CHECK was `BETWEEN 1 AND 24` (code
  points) are copied into the wider tables on startup; nothing references them, so the
  copy is self-contained.
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
  increments the revision and notifies like any other roster change. The exception is
  the host: a join to their own lobby never renames them (it returns their membership
  unchanged), so opening the join link in a second tab can't silently rename them. `POST
  /api/lobbies` takes an empty body (last nickname, or "Host") or `{"nickname": ...}`.
  Responses carry `members[].nickname`, `submitted_by_nickname` on queue entries and
  results, `now_playing.submitted_by_nickname` (null while hidden, like the user ID),
  `scoreboard[].nickname` and `last_result.guesses[].nickname`. Game results are
  stored with user IDs and named on every read, so they always show current names.

## Removing members and ghosts

A ghost is a member who closed their browser (a private window, say) without leaving.
Before this, they kept a game phase open until its timer ran out and counted toward the
skip-vote majority.

- **Host removal**: `DELETE /api/lobbies/memberships/{host membership}/members/{user_id}`
  (see [lobby API](lobby-api.md#remove-a-member)). It works like that member leaving:
  their skip votes stop counting, a game phase waiting only on them closes, and if
  they scored they stay on the scoreboard marked "(left)" (with 0 points they drop
  off it). Their streams end with
  `membership_ended` reason `removed` ("The host removed you from the session. You can
  rejoin with the code."). It is not a ban: they can rejoin with the code and get their
  name back.
- **Presence** (`backend/src/lobbies/presence.rs`): each open event stream counts as
  its account being here. When an account's last stream closes, it is away; after 60
  seconds away it is not waited for by the game's "everyone has acted" check and does
  not count toward the skip-vote guest count. It stays a member (points, songs and name
  stay) and counts again as soon as it reconnects. When the minute passes, the backend
  settles the lobby (recounts the queue, lets the game phase close) and bumps the
  revision so everyone refetches; reconnecting does the same. While skipped, the
  member has `away: true` in `members[]` and the host's people list tags them "away".
  The host is never skipped. Accounts never seen on a
  stream (a member who hasn't opened the lobby page yet, API clients) count as present.
  Presence is kept in memory: after a restart, ghosts count again until they are
  removed. A phone with the screen locked may drop its stream; it counts again once the
  page reconnects.

## Abuse limits

- **New guest accounts per client address**: at most 200 per sliding hour (high enough for a
  class or party sharing one public address, such as campus Wi-Fi), kept in
  memory (`auth::guest::GuestLimiter`, forgotten on restart, which is fine for a
  nuisance limit). The 201st gets **429 `too_many_guests`** with `Retry-After`. Only
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
(#50/#53). Hosts still can't join another lobby without leaving theirs (the join page
asks before ending their session). Signing out still never leaves or closes a lobby.
