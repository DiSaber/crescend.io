# Game modes: plan and contract

Status: working plan on `to-pimp-a-branch`. Tracks #43; uses the TV/phone split from #48 where it exists. Final deliverable is due 2026-11-30.

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
| `Ballot` | How submissions are voted on | `All` (one ballot), `Pairs` (head-to-head) |
| `Winner` | How a round's result is chosen | `MostVotes` (ties: earliest submission), `PairWinners` |
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
| **Duel** | None / Pick / Pairs / PairWinners / Listen / WinnerBonus | Songs face off in pairs; winners play |
| **Name That Tune** | None / Propose / — / — / GuessTitle / GuessSpeed | Players secretly add songs; each round plays one as a clip; everyone races to type the title |
| **Picture This** | Image / Pick / All / MostVotes / Listen / VotesToPicker | Host uploads images and prompts; players pick the song that fits best |

### Name That Tune details

- **Proposals** are secret and come from the normal song search or a pasted link. A proposer can't guess their own song (they sit that round out and score from it instead).
- **Answer key**: the curated starter title when the song has one; otherwise the YouTube title cleaned of a leading "Artist - ", bracketed or parenthesized suffixes ("(Official Video)", "[HQ]") and "ft."/"feat." tails.
- **Matching**: normalized (case, accents, punctuation removed) and forgiving (a small edit distance relative to length, or all significant words present). Exact rules live in tests with real titles.
- **Scoring** (`GuessSpeed`): first correct guess gets the most points, decreasing with time; the proposer earns a bonus for each person who guessed it, so obscure picks aren't automatically best.
- **Hidden title**: players must not see the title or video while guessing. **Open risk:** YouTube's API policies restrict obscuring the embedded player. Check the policy before choosing an approach (for example, a dedicated audio-style view the policy allows) and record the decision here with a citation.

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
  better than a win.

## API (sketch; the backend's OpenAPI docs are the source of truth once built)

All under the existing bearer auth; game routes require an active membership of the lobby; host-only routes return `403 host_required`.

| Method and path | Who | Purpose |
| --- | --- | --- |
| `GET /api/game-modes` | any | Presets and the caller's saved modes, each with its settings schema |
| `POST /api/game-modes` | any | Save a custom mode; validates rules and settings |
| `DELETE /api/game-modes/{id}` | owner | Delete a saved mode |
| `POST /api/lobbies/memberships/{m}/game` | host | Start a game: `{mode_id or custom: {name, rules}, settings, prompts}` |
| `GET /api/lobbies/memberships/{m}/game` | member | Game snapshot: mode, round, phase, `phase_ends_at`, prompt, the caller's own submission, the ballot (only during vote), results, scoreboard, revision |
| `DELETE /api/lobbies/memberships/{m}/game` | host | End the game (scoreboard kept until the next start); on an ended game, dismiss it |
| `POST .../game/advance` | host | `{phase_seq}` skips that phase; `{play_seq}` reports the playing song ended |
| `POST .../game/submissions` | player | Pick or propose a song: `{url, request_id}` (same validation as the queue) |
| `POST .../game/votes` | player | Vote for a submission: `{submission_id}` |
| `POST .../game/guesses` | player | Guess a title: `{text}` → `{correct, points}` |
| `POST .../game/images` | host | Upload an image (multipart) → `{image_id}` |
| `GET .../game/images/{id}` | member | Fetch an image |

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
