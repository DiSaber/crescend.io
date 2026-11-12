# Game API

Games run inside a lobby on one shared engine ([plan](game-modes.md)). All routes
use the existing bearer auth; lobby game routes take the caller's membership
generation like the queue. Responses, including errors, use `Cache-Control: no-store`.
JSON bodies are limited to 4 KiB; unknown and duplicate fields are rejected.
Errors use the shared `{code,error}` envelope ([errors](api-errors.md#game-errors)).

## Modes

`GET /api/game-modes` (bodyless) returns `{"modes":[...]}`. Each mode has `id`
(a preset key such as `party`), `name`, `description`, `preset: true`, its `rules`
(one choice per rule: `prompt`, `collect`, `ballot`, `winner`, `challenge`,
`scoring`) and `settings`, the customize-form schema: a list of
`{key,label,type,default,min,max,step,unit}` where `type` is `integer` or `boolean`
(booleans have null min/max/step). An integer is valid when
`min <= value <= max` and `(value - min) % step == 0`; build the customize form
from this list and only send keys it contains.

| Preset | Rules (ballot / winner / scoring) | Defaults that differ from Party |
| --- | --- | --- |
| `party` | `all` / `most_votes` / `votes_to_picker` | 5 rounds, 45 s collect, 30 s vote, full songs |
| `speed_round` | same as Party | 8 rounds, 20 s collect, 15 s vote, 60 s clips |
| `duel` | `pairs` / `pair_winners` / `winner_bonus` | 3 rounds, 60 s clips; no `votes_per_player` |
| `name_that_tune` | `none` / `none` / `guess_speed` (collect `propose`, challenge `guess_title`) | every proposal, 90 s to propose 2 songs, 30 s guesses from 0:30, 8 s reveal |

Labels and limits depend on the rules: Name That Tune relabels `play_seconds` as
"Guess time" (10–120 s), adds `reveal_seconds` and hides the voting and overlap keys.

## Starting and ending

Hosts start with `POST /api/lobbies/memberships/{membership_id}/game` and exactly
one of `mode_id` or `custom`, plus optional `settings` overrides:

```json
{"mode_id": "duel", "settings": {"rounds": 4, "play_seconds": 45}}
{"custom": {"name": "Vote Duel", "rules": {"prompt": "none", "collect": "pick", "ballot": "pairs",
  "winner": "pair_winners", "challenge": "listen", "scoring": "votes_to_picker"}}, "settings": {"rounds": 0}}
```

`settings` keys must be in the mode's served schema with the right type and range,
otherwise `400 invalid_game_settings`. A `custom` mode starts from Party's defaults;
its name is 1–40 characters after trimming. Rule combinations the engine cannot run
return `400 invalid_game_rules` (`ballot: all` needs `winner: most_votes`;
`ballot: pairs` needs `winner: pair_winners`). The snapshot's `mode_id` is the
preset key or `custom`, and `mode` holds the resolved name, rules and settings.
Guests get `403 host_required`; an unknown preset returns
`404 game_mode_unavailable`; a running game returns `409 game_running`. Starting
replaces an ended game and its scoreboard. The response is
`{"changed":true,"game":<snapshot>}`.

Hosts end with bodyless `DELETE .../game`. A running game becomes `status: ended`
(phase `ended`, songs stop, scoreboard kept). Deleting an ended game dismisses it
(`game: null`, the plain jukebox). With no game it is a `changed: false` no-op.

## Snapshot

Any active member reads `GET .../game` (bodyless):

```json
{
  "membership_id": "...", "lobby_id": "...", "revision": "14",
  "server_time": "2027-01-15T08:00:05Z",
  "game": {
    "id": "...", "mode_id": "party",
    "mode": {"name": "Party", "rules": {...}, "settings": {"rounds": 5, ...}},
    "status": "running", "round": 2, "total_rounds": 5,
    "phase": "collect", "phase_seq": 4, "phase_ends_at": "2027-01-15T08:00:50Z",
    "my_submissions": [{"id": "...", "song": {"video_id": "...", "title": "...", "author_name": "...", "thumbnail_url": "...", "embed_url": "..."}}],
    "submission_count": 3,
    "ballot": null,
    "last_result": {"round": 1, "entries": [{"submission_id": "...", "song": {...}, "submitted_by_user_id": "7", "votes": 2, "winner": true, "points": 20}]},
    "now_playing": {"play_seq": 1, "round": 1, "hidden": false, "song": {...}, "submitted_by_user_id": "7",
      "start_seconds": 0, "started_at": "...", "ends_at": null},
    "guess": null, "up_next_count": 0,
    "scoreboard": [{"user_id": "7", "points": 20, "active": true}]
  }
}
```

`game` is null when no game exists. `revision` is the lobby revision shared with
the queue: every game commit increments it once. Count down to `phase_ends_at`
using `server_time` as the reference, never the device clock; it is null in
phases without a timer (`play`, `ended`). `total_rounds` is null for "until stopped".

Visibility rules: `my_submissions` holds only the caller's own songs for the current
round; others are counted in `submission_count` without identities. `ballot` exists
only during `vote` and lists songs anonymously in `groups` with `mine` and
`votable`, plus `votes_allowed` (how many votes this caller can cast on this ballot)
and the caller's `my_votes`; it never contains counts or submitters. Counts, submitters and points appear in `last_result` only after the
vote closes. `scoreboard` lists active members (including zero scores) and departed
accounts that scored (`active: false`), highest first, ties by account ID.

## Phases (Party, Speed Round, Duel)

`collect` → `vote` → `play`, then the next round's `collect`. `phase_seq` increases
on every change.

- **collect**: each member, host included, submits up to `songs_per_player` songs.
  It closes at `phase_ends_at` or as soon as every active member has submitted all
  their songs. Zero songs skip the round; one song wins without a vote.
- **vote**: one ballot (`ballot.groups[0]`). Each member casts up to
  `votes_per_player` votes, one per song, never for their own unless `self_vote`.
  It closes at `phase_ends_at` or once every member has cast every vote they can.
  The most votes wins; ties go to the earliest submission. `VotesToPicker` gives the
  picker 10 points per vote received.
  In **Duel** (`pairs`) songs pair up in submission order and an odd song out gets a
  bye (`votable: false`). Each member casts one vote per pair, never in the pair
  holding their own song unless `self_vote`. Every pair winner plays, in pair order,
  byes included; `winner_bonus` gives 50 points per contested pair won.
- **play**: the round's winner joins the game's play list. `now_playing` is the
  head of that list; the host's player plays it from `start_seconds`. With
  `play_seconds: 0` (Party) `ends_at` is null and the host reports the end;
  otherwise the server ends it at `ends_at`. With `overlap_next_round` the next
  round's `collect` opens while the winner plays; a round whose winner would have to
  wait behind an earlier round's song stays in `play` until that song ends. Without
  overlap every round waits in `play` for its songs. After the last round the game
  ends once the play list is empty.

## Phases (Name That Tune)

`collect` (once) → `guess` → `reveal`, repeated per round, then `ended`.

- **collect**: everyone, host included, secretly proposes up to `songs_per_player`
  songs through the same submissions route. `my_submissions` lists only the caller's
  own proposals for the whole game; `submission_count` counts all of them. A video
  already proposed in this game returns `409 song_already_proposed`. It closes at the
  timer or when everyone has proposed all their songs; with no proposals the game
  ends. `total_rounds` stays null until it closes, then becomes the number of rounds
  that will play (all proposals, or `rounds` if smaller).
- **guess**: one proposal plays as a clip from `start_seconds` for `play_seconds`
  (`phase_ends_at`). For everyone but the host, `now_playing` is
  `{"hidden": true, "song": null, "submitted_by_user_id": null, ...}`; the host's
  player gets the video and must show it per [the policy decision](game-modes.md#hidden-title-what-youtubes-policies-allow-checked-2026-11-05).
  `guess` in the snapshot is the caller's standing: `eligible` (false for the host
  and the proposer), `own_song`, `correct`, `points`, `wrong_guesses`,
  `guesses_left` and the public `correct_count`. It closes at the timer or once every
  eligible player is right.
- **reveal**: for `reveal_seconds`; `last_result` has `answer` (the title guesses were
  matched against), the song with its proposer and the proposer's points, and
  `guesses` (correct guessers fastest first, with `rank` and `points`). The clip keeps
  playing and is no longer hidden. Then the next round's `guess`, or `ended`.

The host can `advance` either phase; `play_seq` reports a clip that ended early
(guessing continues, `now_playing` becomes null).

The ticker applies due transitions within about a second and publishes them.
Every write also catches the game up first, so a request just after a deadline acts
on the phase that is open now (a late song lands in the next round's collect).
Transitions start the next timer from the moment they are applied.

## Player actions

Submit with `POST .../game/submissions` and `{"url":"https://youtu.be/...","request_id":"<32 hex>"}`,
validated like queue submissions (supported YouTube links, server oEmbed metadata).
Keep a request ID across uncertain retries: a matching retry returns the original
`submission_id` with `changed: false` and no metadata lookup, even after collect
closed; reusing it for another video returns `409 idempotency_conflict`. Outside
`collect` returns `409 game_phase_closed`; over the limit `409 submission_limit_reached`.
Response: `{"changed":true,"submission_id":"...","game":<snapshot>}`.

Vote with `POST .../game/votes` and `{"submission_id":"..."}`. Repeating a vote is a
`changed: false` no-op. A song not on the current ballot returns
`404 game_submission_unavailable`; own songs `409 self_vote_forbidden`; extra votes
`409 vote_limit_reached`; outside `vote` `409 game_phase_closed`.

Guess with `POST .../game/guesses` and `{"text":"bohemian rapsody"}` (1–100
characters after trimming). The response is
`{"changed":true,"correct":true,"points":108,"game":<snapshot>}`. Matching forgives
case, accents, punctuation, small typos and extra words such as the artist (rules in
[the plan](game-modes.md#design-decisions-backend)). Points: 50, plus up to 50 for
time left, plus 25 for the first correct guess. After a correct guess, more guesses
return `changed: false` with the same points. The host gets `403 host_cannot_guess`;
the proposer `409 own_song`; the eleventh wrong guess in a round
`409 guess_limit_reached`; outside `guess` `409 game_phase_closed`.

## Host progression

`POST .../game/advance` takes exactly one of:

- `{"phase_seq": 4}` skips that phase now (a skipped `collect` closes early, a
  skipped `vote` counts the votes so far). Skipping a waiting `play` phase ends the
  song holding it up.
- `{"play_seq": 1}` reports that the song ended on the host player (use it for
  `ends_at: null` songs, or when a clip ends early).

A stale `phase_seq` returns `409 game_phase_changed`; a `play_seq` that is not the
playing song returns `409 game_song_changed`, so a delayed `ended` callback can never
skip its successor. Both fields or neither is `400 invalid_request`. Ended or absent
games return `409 game_not_running`.

## Sync and the jukebox

Game commits publish on the membership SSE stream exactly like queue commits.
On `sync_required` and `lobby_changed` refetch the game as well as the membership
and queue, with the same BigInt revision comparison and in-flight/dirty rules as
[the queue](queue-api.md). The stream's one-second storage reconciliation also
picks up ticker commits from other processes.

While `game.status` is `running`, the host player plays `now_playing` and leaves
the jukebox alone: the queue keeps its current entry and order and stays writable,
so guests can keep adding songs for later. When the game ends (or is dismissed)
the player resumes the queue where it was. The backend does not lock the queue.

Closing the lobby deletes its game; expiry deletes it with the lobby.
