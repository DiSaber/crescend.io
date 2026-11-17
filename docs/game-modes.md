# Game modes: plan and contract

Status: working plan on `to-pimp-a-branch`. Tracks #43; uses the TV/phone split from #48 where it exists. Final deliverable is due 2026-11-30. Backend G1–G5 built on 2026-11-05 (endpoint reference: [game-api.md](game-api.md)). Frontend: G1–G5 built 2026-11-05 (board view at `/lobby/tv/`). G6 (hardening, lifecycle and load tests, TV polish, accessibility, phone checks) built 2026-11-06. G7 (rules decided after QA played every mode) built 2026-11-06.

## Goal

A host can run a music game in their lobby instead of (or after) the plain jukebox. Every game runs on one shared engine. A **game mode** is data: a set of swappable rules plus settings. Hosts customize at four levels:

1. **Pick a preset** (Party, Speed Round, Duel, Name That Tune, Picture This).
2. **Tweak a preset**: change its exposed settings before starting.
3. **Create a mode**: choose rules and settings, save it privately, reuse it.
4. **Custom prompts**: write round prompts with optional host-uploaded images ("Which song is most Mr. Bean?").

The plain jukebox keeps working; it is simply "no game running".

## Engine

A game is a sequence of rounds. Each round moves through phases on server-owned timers:

```
collect -> vote -> play -> (next round)        Party, Duel, Picture This
collect (once) -> [play -> guess -> reveal]*    Name That Tune
```

- The server stores `phase` and `phase_ends_at` (UTC). Phones count down to it; nobody's clock is trusted.
- A background ticker (like the existing cleanup and stream reconciliation loops) advances phases when time runs out. The host can also skip a phase.
- With `overlap_next_round`, the next round's collect phase opens while the current winner plays (the flow from the #43 comment).
- Each change bumps a game revision and publishes on the existing membership event stream, exactly like queue commits. Clients refetch the game on `sync_required` and `lobby_changed`, as they already do for the queue.
- While a game runs, the host's player plays the game's song for the current play phase; the jukebox queue is paused, not lost.
- When the game ends, the final scoreboard stays visible until the host starts another game or returns to the jukebox.

### Rules (the object-oriented part)

Each mode picks one implementation of each rule. In Rust each is a trait with a few implementations, selected by an enum stored in the mode definition. New behavior means one new implementation; everything else is shared.

| Rule | Decides | Implementations |
| --- | --- | --- |
| `Prompt` | What a round asks for | `None`, `Text`, `Image` (text + host image) |
| `Collect` | What players submit | `Pick` (a song per round, visible after collect), `Propose` (songs up front, kept secret) |
| `Ballot` | How submissions are voted on | `All` (one ballot), `Pairs` (head-to-head, interleaved by player) |
| `Winner` | How a round's result is chosen | `MostVotes` (ties: earliest submission), `PairWinners` (a tie plays the earliest song, scoring nothing) |
| `Challenge` | What players do during play | `Listen` (nothing), `GuessTitle` (Name That Tune) |
| `Scoring` | Who earns points | `VotesToPicker`, `WinnerBonus`, `GuessSpeed` |

Invalid combinations (e.g. `Propose` with `Ballot::Pairs`) are rejected when a mode is saved or started.

### Settings

Each mode exposes a list of settings. Each setting has a key, label, type, default, min, max, step and unit. The server serves this schema, the customize form is generated from it, and the server enforces the same limits on start. Common settings:

| Key | Meaning | Default (Party) |
| --- | --- | --- |
| `rounds` | Rounds, or 0 for "until stopped" | 5 |
| `collect_seconds` | Time to pick a song | 45 |
| `vote_seconds` | Voting window | 30 |
| `play_seconds` | How much of the winner plays; 0 = full song | 0 |
| `clip_start_seconds` | Where clips start (skips intros) | 0 |
| `votes_per_player` | Votes each player can cast | 1 |
| `songs_per_player` | Submissions per round | 1 |
| `self_vote` | Can players vote for their own song | false |
| `overlap_next_round` | Next round collects while the winner plays | true |

Song length is a mode decision: presets set `play_seconds`, and hosts can change it at selection.

## Presets

| Preset | Rules | Notes |
| --- | --- | --- |
| **Party** | None / Pick / All / MostVotes / Listen / VotesToPicker | #43's flow: pick, vote, winner plays, overlap |
| **Speed Round** | Same as Party | 20 s collect, 15 s vote, 60 s clips |
| **Duel** | None / Pick / Pairs / PairWinners / Listen / WinnerBonus | Songs face off in pairs; winners play. Needs 3+ players |
| **Name That Tune** | None / Propose / — / — / GuessTitle / GuessSpeed | Players secretly add songs; each round plays one as a clip; everyone races to type the title |
| **Picture This** | Image / Pick / All / MostVotes / Listen / VotesToPicker | Host uploads images and prompts; players pick the song that fits best |

### Name That Tune details

- **Proposals** are secret and come from the normal song search or a pasted link. A proposer can't guess their own song (they sit that round out and score from it instead).
- **Answer key**: the curated starter title when the song has one; otherwise the YouTube title cleaned of a leading "Artist - ", bracketed or parenthesized suffixes ("(Official Video)", "[HQ]") and "ft."/"feat." tails. Either way bracketed parts and "ft." tails of the key are optional when matching, so "Old Town Road" names "Old Town Road (Remix)" (G7).
- **Matching**: normalized (case, accents, punctuation removed) and forgiving (a small edit distance relative to length, or all significant words present). Exact rules live in tests with real titles.
- **Scoring** (`GuessSpeed`): first correct guess gets the most points, decreasing with time; the proposer earns a bonus for each person who guessed it, so obscure picks aren't automatically best.
- **Hidden title**: players must not see the title or video while guessing. Decision below.

#### Hidden title: what YouTube's policies allow (checked 2026-11-05)

Sources: [YouTube API Services Developer Policies](https://developers.google.com/youtube/terms/developer-policies)
and [Required Minimum Functionality](https://developers.google.com/youtube/terms/required-minimum-functionality) (RMF).

Not allowed, so the frontend must not do any of these to an embedded player:

- **Covering or blurring it.** RMF "Overlays and frames": you "must not display overlays, frames, or other visual elements in front of" any part of the player. Developer Policies III.I.4: "must not remove, obscure, alter, or disable any links that appear in YouTube players" (the title is such a link).
- **Altering or blocking it.** III.I.6: do not "modify, build upon, or block any portion or functionality of a YouTube player". RMF "YouTube Player attributes": no changes not described in the API documentation.
- **Audio-only tricks.** III.I.7: do not "separate, isolate, or modify the audio or video components". So no zero-size, off-screen or transparent player used to play only the sound.
- **Shrinking it.** RMF "Embedded YouTube Player size": a viewport "at least 200px by 200px".
- **Playing it hidden.** III.I.9: no features that play content "from a background player". RMF autoplay: scripted playback waits until "more than half of the player is visible".

Allowed: a normal, visible, unobscured player of at least 200×200 px, started with the
documented `start` parameter or `seekTo` for `clip_start_seconds`, and stopped with
the IFrame API. Nothing in the policies requires every screen in the room to show the
player, so phones simply don't embed one.

**Recommended approach (built into the backend):** the host's device is the DJ. It
plays the clip in an ordinary visible player, the only screen that gets the video
during guessing; the backend sends guessers `now_playing.hidden: true` with no song,
title or proposer, and the host never guesses (`403 host_cannot_guess`). Phones show
only the guess box, timer and scoreboard. The shared TV view (#48) shows the game
board, not the player, during `guess`. Because the player's own title bar can't be
hidden, the setup screen should tell the host to keep the playing screen out of
players' sight (a laptop or phone on a speaker works); if the host plays on the TV
everyone watches, the title will be visible and the round is honor-system. During
`reveal` the clip keeps playing and everyone may see it.

## Saved modes and tracking (level 3)

- A saved mode is private to the account that created it: name, rule choices, settings, and optional default prompts.
- Every mode created or started is also logged (account, time, definition, start count) so popular creations can later become presets. The log is not exposed through the API.

## Prompts and images (level 4)

- A game can carry a prompt list: text, plus an optional image. Prompts are used in order, one per round.
- **Images are host-only.** Accepted: JPEG, PNG or WebP, up to 5 MB and 4096 px on a side. The server re-encodes them (which strips metadata such as location) and stores them on the backend's data volume.
- Images are served only to members of that lobby and deleted when the lobby closes or expires. Limit 30 images per lobby.
- The host sees each image in the prompt editor before starting; nothing is shown to players before the round uses it.

## Design decisions (backend)

Recorded as each milestone lands; [game-api.md](game-api.md) is the endpoint reference.

- **Jukebox coexistence (G1).** A game is one row per lobby next to the queue, not a
  change to it. While a game runs the host player plays the game's `now_playing` and
  does not advance the queue, so the queue's current entry and order are kept, and it
  stays writable for later songs. Ending or dismissing the game hands playback back.
  The backend never locks the queue; "paused" is the player's choice of source.
- **One revision (G1).** Game commits increment the existing lobby revision, so the
  membership event stream needs no new event type and every hint means "refetch the
  membership, queue and game".
- **Ticker (G1).** A one-second loop (`game::start_ticker`) applies due transitions
  using the auth clock; tests inject the clock and call `tick_games` directly. Every
  write also catches its game up first in the same `BEGIN IMMEDIATE` transaction, so
  correctness never depends on the ticker's timing.
- **Play list (G1).** Decided songs go to a per-game play list; `now_playing` is its
  head. Overlap means the next round collects while an earlier round's songs play,
  but never more than one round ahead, so the list cannot grow without bound.
- **Everyone plays (G1).** The host picks and votes like guests in pick modes.
  Phases close early once every active member has done everything they can.
- **Rules (G1).** Each rule is a serde enum stored in the mode definition; each
  variant returns a trait object (`Ballot::groups`, `Winner::winners`,
  `Scoring::round_points`). Points: `VotesToPicker` is 10 per vote received.

- **Settings schema (G2).** `GET /api/game-modes` serves each mode's settings as
  `{key,label,type,default,min,max,step,unit}` with `type` `integer` or `boolean`.
  The engine keeps one fixed settings struct; a rule set only decides which keys are
  exposed (pair ballots hide `votes_per_player`). The start request sends overrides
  as `{key: value}`; the server applies the same schema (exposed key, exact type,
  range, step) and rejects anything else with `invalid_game_settings`.
- **Rule validation (G2).** `Rules::validate` names the first conflict. Today:
  `all` ballots need `most_votes`, `pairs` ballots need `pair_winners`. Later
  milestones add their own pairings. Hosts can also start an unsaved `custom` mode
  (name plus rules) on Party's default settings.
- **Duel (G2).** Pairs form in submission order with a bye for an odd song; a player
  sits out the pair holding their own song. Every pair winner plays. `WinnerBonus` is
  50 points per contested pair won; byes play but score nothing, so a bye is never
  better than a win. (G7 changed pairing, ties and the player minimum: see below.)

- **Name That Tune flow (G3).** One `collect` for proposals (default 2 each, 90 s),
  then per round `guess` (the clip plays hidden for `play_seconds`, default 30 from
  `clip_start_seconds` 30) and `reveal` (`reveal_seconds`, default 8, clip plays on).
  Play order is round-robin by proposer, so everyone's first song plays before anyone's
  second; `rounds: 0` plays every proposal, otherwise the count is capped by it. The
  same video can be proposed once per game. `Propose` requires ballot and winner
  `none`, challenge `guess_title` and scoring `guess_speed`, and the reverse.
- **Who guesses (G3).** Everyone except the host (their player shows the video) and
  the round's proposer. Guessing closes early once every eligible player is right.
  Each player gets 10 wrong guesses per round (G7: only wrong guesses count, and a
  right answer is still accepted after the tenth).
- **Answer keys (G3).** Stored at proposal time: the curated starter title when the
  video is in the starter catalog, else the YouTube title with HTML entities decoded,
  bracketed parts removed, the tail from " ft."/" feat."/" featuring " cut, split on
  " - ", " | " and similar separators, segments that are only packaging ("Official
  Music Video", "Lyrics", "4K") dropped, and the second remaining segment taken as the
  title (the first is the artist). Strong packaging words alone mark a segment, so
  songs called "Clean" or "Music" survive. Cases live in `game/answer.rs` tests.
- **Matching (G3).** Both sides are lowercased with accents, apostrophes and
  punctuation removed. A guess is right if, ignoring spaces, it is within 0 edits
  (answers of 1-3 letters), 1 (4-8), 2 (9-14) or length/7 (longer) of the answer, or
  if it contains every significant answer word (stopwords like "the", "of" optional;
  words of 5+ letters may have one typo) with at most three extra words, so "queen
  bohemian rhapsody" counts but a list of titles does not. Each guess word counts
  for one answer word, so a repeated word must be typed as often as the title has
  it: "Waka" or "Waka Flocka" does not name "Waka Waka", nor "Cha Slide" "Cha-Cha
  Slide".
- **`GuessSpeed` points (G3).** A correct guess earns 50 plus up to 50 in proportion
  to the time left, and the first correct guesser adds 25 (so 125 at best, 50 at the
  buzzer). The proposer earns 20 per correct guesser at the reveal: a song nobody
  knows scores its proposer nothing.

- **Prompts (G4).** `PromptRule` gains `text` (text required, no image) and `image`
  (uploaded image required, text optional). Prompts are part of the start request
  (1-50, text up to 200 characters) and stored with the game; only the current
  round's prompt is sent, and players can fetch only the images of rounds the game
  has reached. Rounds default to one per prompt. `Propose` modes take no prompts.
- **Images (G4).** New crates: `image` (JPEG, PNG and WebP decoders only) and axum's
  `multipart` feature. Limits: 5 MB, 4096 px a side, a 256 MB decode allocation cap,
  30 per lobby. The upload is authorized before its body is read, decoded on a
  blocking thread, rotated per EXIF and re-encoded (PNG if it has alpha, else JPEG
  at quality 85), which strips all metadata. Files are stored in
  `game-images/<lobby_id>/<image_id>.<ext>` beside `data.db` on the data volume and
  owned by `game_images` rows. A host leaving deletes the rows in the closing
  transaction and the folder right after; the minute cleanup loop removes folders of
  closed or expired lobbies, which also covers a crash between the two.

- **Saved modes (G5).** `game_modes` rows hold the owner, the resolved definition
  (name, rules, concrete settings) and default prompts. Only `text` modes keep
  default prompts, because uploaded images are deleted with their lobby. 50 per
  account. Starting one applies the start request's overrides on top. Deleting a
  foreign or absent ID is a silent no-op so IDs cannot be probed.
- **Creation log (G5).** `game_mode_log` is append-only and internal (no route reads
  it). One row per mode saved (`created`) and per game started (`started`, for
  presets, custom and saved modes alike): account, time, mode key (preset key,
  `custom` or saved ID), the full definition and `definition_hash`, a SHA-256 of
  the rules and settings without the name, so renamed copies count together. Rows
  survive mode deletion. Popular creations to consider as presets:

  ```sql
  SELECT definition_hash, count(*) AS starts, count(DISTINCT account_id) AS hosts,
         max(definition) AS example
  FROM game_mode_log WHERE event = 'started' AND mode_key NOT IN
    ('party', 'speed_round', 'duel', 'picture_this', 'name_that_tune')
  GROUP BY definition_hash ORDER BY hosts DESC, starts DESC LIMIT 20;
  ```

## Hardening (G6)

- **A guest leaving settles the game** in the same commit as the leave. If everyone
  still in the lobby has picked, voted or guessed, the phase closes now instead of at
  its timer, so a round never stalls on players who left. Departed players keep
  their picks and stay on the scoreboard (marked inactive) when they scored.
- **The host leaving** closes the lobby and deletes the game with it; guests get
  `403 membership_forbidden` on every game route and the ticker has nothing to run.
- **Lobby expiry mid-round** freezes the game: from the expiry instant reads, writes
  and the ticker all stop, and the cleanup sweep deletes it.
- **Deadlines are exclusive.** An action at exactly `phase_ends_at` is refused
  (`game_phase_closed`, or `game_phase_changed` for a skip) and its catch-up rolls
  back with it; the ticker (every second) publishes the change. A host skip racing
  the timer therefore never skips two phases.
- **A stalled ticker** catches up in one commit per lobby, but each phase it enters
  gets a full timer from the moment it wakes, so a reveal still shows for its
  `reveal_seconds` instead of flashing past.
- **Double starts**: concurrent starts serialize; exactly one wins and the rest get
  `409 game_running`.
- **Load**: twenty players picking, reading and voting at once over HTTP all succeed,
  every commit bumps the revision exactly once, and every vote counts once. SQLite's
  busy and pool waits are 5 s (were 1 s): twenty serialized writes in one second can
  take longer than a second on a slow host, and a 1 s wait answered some players 503.
- **Guest limit**: guest sign-in allows 200 new guest accounts per address per hour
  ([guest join](guest-join.md#abuse-limits)), raised from 20 because a class on one
  Wi-Fi shares one public address. Returning guests and Google users are not limited.
- **TV board**: the root font scales with the screen (16 px at 1280x720, 24 px at
  1080p, 48 px at 4K) and the board fills exactly one screen. The prompt image takes
  the height left over, and sits beside the prompt text while voting; 7+ ballot songs
  use compact cards and 13+ use four columns; 9+ players use compact scoreboard rows
  and 13+ two columns. Checked with 20 players at 720p, 1080p and 4K.
- **Accessibility**: one polite live region per game view announces each phase, the
  answer and round winners; countdowns are labelled timers that announce 30, 10 and
  5 seconds left and time up; every control shows a focus ring; secondary text uses
  zinc-400 (about 7:1 on the cards); the lobby and board have a `main` landmark and an
  `h1`. axe reports no violations on the lobby, setup, each phase and the board.

## QA rules (G7)

Four reviewers played full games of every preset on 2026-11-06. Every round
completed; these are the rules decided from what they found.

**Duel**

- **Ties score nothing.** A pair that ends level, 0-0 included, plays its earliest
  song but awards no points; the result marks both songs `tie` ("Tie, no points").
  Only a song with strictly more votes wins the 50-point bonus. A bye is marked
  `bye`, never "Winner". Results carry each song's `group` (pair) and `outcome`
  (`won`, `lost`, `tie`, `bye`), so the round results and the TV board list the
  songs pair by pair and say which beat which.
- **Shared wins.** When several players share the top score, the final screen, the
  TV board and the announcer name all of them ("Tie: Ana and Bo share the win").
- **Pairing.** Songs are interleaved by player (everyone's first song, then
  everyone's second) and each is paired with the next song from someone else, so
  with `songs_per_player` above 1 nobody faces themselves. A self-pair happens only
  when one player's songs are all that is left.
- **Three players minimum.** With two, each player's song is in the only pair, so
  nobody can vote. Starting a `pairs` mode with fewer than three members returns
  `409 not_enough_players`; setup warns and disables Start.
- **Own pair.** A vote for the other song in your own pair returns `409 own_pair`
  ("Your song is in this pair") instead of the self-vote error.
- **Empty rounds and small Duels.** A round nobody picked a song for says "Round 1:
  nobody picked a song." on phones, the TV board and the announcer. When a running
  Duel drops below 3 members, phones and the board show "Duel needs 3 players for
  voting; remaining pairs are ties until more join."

**Name That Tune**

- **Optional parts.** Curated starter titles go through the same cleanup as YouTube
  titles: brackets and "ft."/"feat." tails are optional on both the key and the
  guess, so "Old Town Road", "Dracula" and "A Bar Song" count, as do the full titles.
- **Nobody to guess.** When only the host and the proposer remain, the round is
  skipped: no clip plays and a 3-second reveal says "Skipped: nobody left to guess".
  Setup warns when the lobby has fewer than 3 players.
- **Guess limit.** Only wrong guesses count toward the 10. A wrong guess past the
  limit is refused without counting; a right one is accepted. The phone shows the
  remaining count from the server snapshot; at zero the box stays usable and says
  "No wrong guesses left: only the right answer counts now."
- **Ending.** Ending mid-guess closes the round first (the proposer scores for anyone
  who named it) and the final screen reveals the song (`note: ended_early`). A game
  with no proposals ends with "Nobody proposed a song." (`note: no_proposals`).
- **Board never gets the answer.** The TV board reads the game with `?view=board`,
  which returns the guest-visible snapshot even on the host's account: no video or
  title while guessing and no own proposals, so the answer is not even in its
  network responses.
- **Phone boxes.** The guess and pick boxes start empty on every new phase. A
  `502 metadata_unavailable` pick or proposal is a definite rejection ("That video
  can't be used. Try another link or search."), the box stays usable, and pick
  errors clear as soon as the player edits.

**Party and engine**

- **One copy per round.** In pick modes the same video can be picked once per
  round: the second gets `409 song_already_picked` ("Someone already picked that song
  this round. Pick another.").
- **Overlap headings.** With `overlap_next_round`, the play heading names the round
  of the song actually playing (`now_playing.round`), and a newer round's results
  are labelled "Up next" until its songs play.
  During `play` the phones and the TV board also show that song's prompt
  (`now_playing.prompt`) instead of the current round's.
- **Points per song.** `points` on each result entry is what that song earned, so a
  player with two songs sees each one's share; the scoreboard adds them up.
- **Jukebox resume.** The host's jukebox player saves its position each second (and
  when a game takes over the player) per queue entry in session storage. After a
  game the jukebox stays paused behind the Game over card and resumes from that
  position only when the host presses "Back to the jukebox".

**Customize form and My modes**

- **+/-** move one step in the pressed direction from an off-step value (47 with
  steps of 5 goes to 50 or 45) and both bring out-of-range values back inside (400
  goes to the top of the range with either button, so neither is disabled then);
  a button is disabled only at its own end of the range.
- **Summary.** The line next to Start names the fields to fix. Server errors clear on
  Reset, on any edit and after a successful save.
- **Mode names** follow the [nickname character rules](guest-join.md#nicknames),
  one shared validator (`nicknames::display_name`) with a 40-character limit:
  trimmed, inner whitespace runs collapsed, 1-40 user-perceived characters
  (grapheme clusters, so a ZWJ family or flag emoji is one), no control characters
  (U+0000, newlines, U+2028/2029 included), soft hyphens, zero-width or
  bidirectional characters, and joiners or variation selectors only inside emoji
  (`400 invalid_mode_name`). The form applies the same rules, trims before cutting a
  paste at 40 and says when it cut. A name the account already uses, or a preset's
  name, gets " 2", " 3" appended (ignoring case), shortened by whole characters to
  fit 40; a preset clash says "That name is taken by a built-in mode, so this one is
  saved as “Duel 2”." The "Saved" notice clears on Reset and Start, and typing a
  name clears a start rejection.
- **Duplicate settings keys** in a start or save body (`{"rounds": 3, "rounds": 50}`)
  are `400 invalid_request`, instead of the last value silently winning.

## API (sketch; the backend's OpenAPI docs are the source of truth once built)

All under the existing bearer auth; game routes require an active membership of the lobby; host-only routes return `403 host_required`.

| Method and path | Who | Purpose |
| --- | --- | --- |
| `GET /api/game-modes` | any | Presets and the caller's saved modes, each with its settings schema and default prompts |
| `POST /api/game-modes` | any | Save a custom mode `{name, rules, settings, prompts}`; validates rules, settings and prompts |
| `DELETE /api/game-modes/{id}` | owner | Delete a saved mode |
| `POST /api/lobbies/memberships/{m}/game` | host | Start a game: `{mode_id or custom: {name, rules}, settings, prompts}` |
| `GET /api/lobbies/memberships/{m}/game` | member | Game snapshot: mode, round, phase, `phase_ends_at`, prompt, the caller's own submission, the ballot (only during vote), results, scoreboard, revision |
| `DELETE /api/lobbies/memberships/{m}/game` | host | End the game (scoreboard kept until the next start); on an ended game, dismiss it |
| `POST .../game/advance` | host | `{phase_seq}` skips that phase; `{play_seq}` reports the playing song ended |
| `POST .../game/submissions` | player | Pick or propose a song: `{url, request_id}` (same validation as the queue) |
| `POST .../game/votes` | player | Vote for a submission: `{submission_id}` |
| `POST .../game/guesses` | player | Guess a title: `{text}` → `{correct, points}` |
| `POST .../game/images` | host | Upload an image (multipart field `image`) → `{image_id, content_type, width, height, bytes}` |
| `GET .../game/images` | host | List the lobby's images |
| `GET .../game/images/{id}` | member | Fetch an image (non-hosts: only images of rounds already reached) |
| `DELETE .../game/images/{id}` | host | Delete an image the running game does not use |

The snapshot never includes other players' unrevealed submissions, Name That Tune answers before reveal, or vote counts before the vote closes.

## Frontend

- **Host:** a "Start a game" panel in the lobby: preset cards → customize form (generated from the schema) → prompt editor with image upload and preview (Picture This, custom modes) → optional "Save as my mode" → Start.
- **Everyone:** one game view that renders by phase: collect (song search reused from the lobby), vote, play (now playing plus timer), guess (text box plus feedback), reveal, and a scoreboard visible throughout.
- **TV/host screen:** the shared view shows the prompt and image, timers, results and scoreboard; phones show only what the player acts on (#48).
- Existing lobby, queue and search code from #55 and #56 is reused rather than duplicated.

## Build order

| Milestone | Backend | Frontend | Target |
| --- | --- | --- | --- |
| G1 | Engine, phases, ticker, Party preset, `VotesToPicker`, scoreboard, API, tests | Start-a-game (preset only), phase views, scoreboard | 11-12 |
| G2 | Settings schema and validation; Speed Round and Duel | Customize form | 11-15 |
| G3 | Name That Tune: proposals, answer keys, matching, `GuessSpeed` | Guess view; hidden-title playback per the policy decision | 11-19 |
| G4 | Prompts, image upload and storage, Picture This | Prompt editor with upload and preview; prompt display | 11-22 |
| G5 | Saved modes and creation log | "Save as my mode", "My modes" | 11-25 |
| G6 | Hardening, load and lifecycle tests | TV view polish, accessibility, phone checks | 11-28 |

Each milestone lands on `to-pimp-a-branch` first, then goes to `main` as a reviewable PR (backend changes reviewed by @DiSaber, frontend by @kes-bruh and @lanewest).

## Testing

- Backend: unit tests for each rule implementation, answer-key cleanup and matching (with real titles), and the phase machine with a controllable clock. HTTP tests per endpoint, including who can see what and when. No test calls YouTube.
- Frontend: Playwright runs with a host and two or more guests through each preset at phone and desktop widths, with screenshots and a GIF per milestone.
- Song search during games uses the live quota like the lobby does; tests use mocks or the catalog.
