"use client";

import { useState, type FormEvent, type ReactNode } from "react";
import SongPicker from "../SongPicker";
import { Countdown, GameImage, Scoreboard, SongRow, phaseName, playerName, roundLabel } from "./bits";
import { advanceGame, castVote, endGame, guessTitle, submitSong, type GameMutation, type GameView, type RoundResult } from "../../lib/game";
import { newRequestId, QueueError } from "../../lib/queue";
import type { QueueSession, SessionSnapshot } from "../../lib/queue-state";

type Attempt = { url: string; request_id: string };
type Feedback = { text: string; correct: boolean; points: number } | null;

/**
 * One game view for everyone, rendered by phase (docs/game-api.md). Players act here;
 * the host also gets skip and end controls.
 */
export default function GamePanel({ session, state, onNewGame, player }: {
  session: QueueSession; state: SessionSnapshot; onNewGame: () => void;
  /** The host's game player, shown right under the game card. */
  player?: ReactNode;
}) {
  const game = state.game?.game;
  const membershipId = state.membership?.membership_id ?? undefined;
  const ownerId = state.membership?.lobby?.owner_user_id;
  const me = state.userId;
  const isHost = session.isHost();
  const offset = state.clockOffset;
  const [pick, setPick] = useState("");
  const [attempt, setAttempt] = useState<Attempt | null>(null);
  const [uncertain, setUncertain] = useState(false);
  const [guess, setGuess] = useState("");
  const [feedback, setFeedback] = useState<{ phaseSeq: number; value: Feedback }>({ phaseSeq: -1, value: null });
  if (!game || !membershipId) return null;
  const busy = state.gameBusy;
  const name = (id: string | null | undefined) => playerName(id, ownerId, me);

  async function submit(url: string, retry?: Attempt) {
    if (busy && !retry) return;
    const next = retry ?? { url, request_id: newRequestId() };
    setAttempt(next); setUncertain(false);
    try {
      await session.gameMutate("Adding your song…", m => submitSong(m, next.url, next.request_id));
      setAttempt(null); setPick("");
    } catch (error) {
      // Network or server trouble may hide a stored pick; retry with the same request ID.
      const unsure = !(error instanceof QueueError) || error.status >= 500;
      setUncertain(unsure); if (!unsure) setAttempt(null);
    }
  }
  async function sendGuess(e: FormEvent) {
    e.preventDefault();
    const text = guess.trim();
    if (!text || busy || !game) return;
    const phaseSeq = game.phase_seq;
    try {
      const result = await session.gameMutate("Checking…", m => guessTitle(m, text));
      if (!result) return;
      setFeedback({ phaseSeq, value: { text, correct: result.correct, points: result.points } });
      if (!result.correct) setGuess("");
    } catch { /* gameError shows the server's reason. */ }
  }
  const act = (label: string, op: (m: string) => Promise<GameMutation>) => {
    void session.gameMutate(label, op).catch(() => {});
  };
  const skip = () => act("Skipping…", m => advanceGame(m, { phase_seq: game.phase_seq }));
  const end = () => {
    if (game.status === "running" && !window.confirm("End the game for everyone? The scoreboard stays.")) return;
    act(game.status === "running" ? "Ending…" : "Closing…", m => endGame(m));
  };

  const running = game.status === "running";
  const propose = game.mode.rules.collect === "propose";
  const perPlayer = Number(game.mode.settings.songs_per_player ?? 1);
  const playing = game.now_playing && !game.now_playing.hidden && game.now_playing.song ? game.now_playing : null;
  const shownFeedback = feedback.phaseSeq === game.phase_seq ? feedback.value : null;

  return (
    <section aria-label="Game" className="w-full max-w-4xl flex flex-col gap-4">
      <div className="bg-zinc-900 p-4 sm:p-6 rounded-xl border border-[#5C172E] flex flex-col gap-4">
        <header className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <p className="text-xs uppercase tracking-wider text-zinc-400">{roundLabel(game)}</p>
            <h2 className="text-2xl font-extrabold">{phaseName(game)}</h2>
          </div>
          <Countdown endsAt={game.phase_ends_at ?? (game.phase === "play" ? game.now_playing?.ends_at ?? null : null)} offset={offset} />
        </header>

        {game.prompt && (
          <div className="rounded-lg bg-zinc-800 p-3 flex flex-col gap-3">
            <p className="text-xs text-zinc-400">Prompt {game.prompt.number}</p>
            {game.prompt.image_id && (
              <GameImage membershipId={membershipId} imageId={game.prompt.image_id} alt={game.prompt.text ?? "Round image"} className="w-full max-h-[60vh] rounded-lg object-contain bg-black min-h-40" />
            )}
            {game.prompt.text && <p className="text-xl font-semibold">{game.prompt.text}</p>}
          </div>
        )}

        {game.phase === "collect" && (
          <div className="flex flex-col gap-3">
            <p className="text-sm text-zinc-300">
              {propose
                ? `Secretly add up to ${perPlayer} song${perPlayer > 1 ? "s" : ""}. Nobody sees them until they play, and you can't guess your own.`
                : `Add up to ${perPlayer} song${perPlayer > 1 ? "s" : ""} for this round${game.prompt ? " that fit the prompt" : ""}. Everyone votes next.`}
            </p>
            {game.my_submissions.length > 0 && (
              <ul className="flex flex-col gap-2" aria-label={propose ? "Your secret songs" : "Your picks"}>
                {game.my_submissions.map(s => <li key={s.id}><SongRow song={s.song} note={propose ? "your secret pick" : "your pick"} /></li>)}
              </ul>
            )}
            {game.my_submissions.length < perPlayer ? (
              <SongPicker
                id="game-pick"
                value={pick}
                onChange={setPick}
                onSubmit={url => void submit(url)}
                addingUrl={busy && attempt ? attempt.url : undefined}
                disabled={!!busy || !!attempt}
                inputDisabled={!!attempt && !uncertain}
                label={propose ? "Search or paste a secret song" : "Search or paste your pick"}
                submitLabel={propose ? "Propose" : "Pick"}
              />
            ) : <p className="text-sm text-green-400" role="status">You&apos;re in. Waiting for the others…</p>}
            {attempt && uncertain && !busy && (
              <div className="text-sm text-zinc-300">
                <p>Your pick may have gone through. Retrying keeps the same submission ID.</p>
                <button onClick={() => void submit(attempt.url, attempt)} className="min-h-11 px-3 underline">Retry pick</button>
                <button onClick={() => { setAttempt(null); setUncertain(false); }} className="min-h-11 px-3 underline">Pick something else</button>
              </div>
            )}
            <p className="text-sm text-zinc-400" role="status">{game.submission_count} song{game.submission_count === 1 ? "" : "s"} in so far</p>
          </div>
        )}

        {game.phase === "vote" && game.ballot && <Ballot game={game} busy={!!busy} onVote={id => act("Voting…", m => castVote(m, id))} />}

        {game.phase === "guess" && game.guess && (
          <div className="flex flex-col gap-3">
            {isHost ? (
              <p className="rounded-lg bg-amber-950/60 border border-amber-800 p-3 text-sm text-amber-200">
                You&apos;re the DJ this round. Your player shows the title, so keep this screen out of the players&apos; sight and let the board view do the showing.
              </p>
            ) : game.guess.own_song ? (
              <p className="rounded-lg bg-zinc-800 p-3">This one is your song. Sit tight: you get 20 points for everyone who names it.</p>
            ) : game.guess.correct ? (
              <p className="rounded-lg bg-green-950 border border-green-700 p-4 text-lg font-semibold text-green-300" role="status">
                Correct! +{game.guess.points} points
              </p>
            ) : (
              <form onSubmit={sendGuess} className="flex flex-col sm:flex-row gap-3">
                <input
                  value={guess}
                  onChange={e => setGuess(e.target.value)}
                  maxLength={100}
                  autoComplete="off"
                  aria-label="Your guess: the song's title"
                  placeholder="Type the song's title"
                  disabled={!game.guess.eligible || game.guess.guesses_left <= 0}
                  className="min-w-0 flex-1 p-3 text-lg rounded-lg bg-zinc-800 border border-zinc-700 placeholder-zinc-500 focus:outline-none focus:border-purple-500"
                />
                <button type="submit" disabled={!guess.trim() || !!busy || game.guess.guesses_left <= 0} className="min-h-11 px-6 rounded-lg bg-[#5C172E] hover:bg-[#FF0048] disabled:opacity-50 transition font-bold">
                  Guess
                </button>
              </form>
            )}
            {!isHost && !game.guess.correct && shownFeedback && !shownFeedback.correct && (
              <p role="status" className="text-amber-300">“{shownFeedback.text}” isn&apos;t it. {game.guess.guesses_left} guess{game.guess.guesses_left === 1 ? "" : "es"} left.</p>
            )}
            <p className="text-sm text-zinc-400" role="status">{game.guess.correct_count} player{game.guess.correct_count === 1 ? " has" : "s have"} named it</p>
          </div>
        )}

        {game.phase === "reveal" && game.last_result && <Reveal result={game.last_result} name={name} me={me} />}

        {playing && game.phase !== "reveal" && (
          <div>
            <p className="text-xs text-zinc-400 mb-1">Now playing{game.up_next_count > 0 && ` · ${game.up_next_count} more decided`}</p>
            <SongRow song={playing.song!} note={`picked by ${name(playing.submitted_by_user_id)}`} />
            {!isHost && <p className="mt-1 text-xs text-zinc-500">Sound plays on the host&apos;s device.</p>}
          </div>
        )}
        {game.phase === "play" && !playing && <p className="text-sm text-zinc-400">Waiting for the song to start…</p>}

        {game.last_result && game.phase !== "reveal" && !propose && <RoundResults result={game.last_result} name={name} />}
        {game.last_result && game.phase === "ended" && propose && <Reveal result={game.last_result} name={name} me={me} />}

        {game.phase === "ended" && (
          <p className="text-lg">
            {game.scoreboard[0] && game.scoreboard[0].points > 0
              ? <>Winner: <strong>{name(game.scoreboard[0].user_id)}</strong> with {game.scoreboard[0].points} points.</>
              : "No points this time."}
          </p>
        )}

        {state.gameError && <p role="alert" className="text-red-400">{state.gameError}</p>}
        {busy && <p role="status" className="text-sm text-zinc-400">{busy}</p>}

        {isHost && (
          <div className="flex flex-wrap gap-2 border-t border-zinc-800 pt-3">
            {running ? <>
              <button onClick={skip} disabled={!!busy} className="min-h-11 px-4 rounded-lg bg-zinc-700 hover:bg-zinc-600 disabled:opacity-50">Skip {game.phase === "play" ? "song" : "phase"}</button>
              <button onClick={end} disabled={!!busy} className="min-h-11 px-4 rounded-lg text-red-300 hover:bg-red-950 disabled:opacity-50">End game</button>
            </> : <>
              <button onClick={onNewGame} className="min-h-11 px-4 rounded-lg bg-[#5C172E] hover:bg-[#FF0048] font-semibold">New game</button>
              <button onClick={end} disabled={!!busy} className="min-h-11 px-4 rounded-lg bg-zinc-700 hover:bg-zinc-600 disabled:opacity-50">Back to the jukebox</button>
            </>}
          </div>
        )}
      </div>
      {player}
      <Scoreboard game={game} ownerId={ownerId} me={me} />
    </section>
  );
}

function Ballot({ game, busy, onVote }: { game: GameView; busy: boolean; onVote: (id: string) => void }) {
  const ballot = game.ballot!;
  const pairs = game.mode.rules.ballot === "pairs";
  const left = ballot.votes_allowed - ballot.my_votes.length;
  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-zinc-300" role="status">
        {ballot.votes_allowed === 0 ? "Nothing for you to vote on this round." : left > 0 ? `${left} vote${left > 1 ? "s" : ""} left${pairs ? ": pick one song in each pair" : ""}.` : "Votes in. Waiting for the others…"}
      </p>
      {ballot.groups.map((group, g) => {
        const votedHere = group.entries.some(e => ballot.my_votes.includes(e.submission_id));
        return (
          <div key={g} className={pairs ? "rounded-lg border border-zinc-800 p-2 flex flex-col gap-2" : "flex flex-col gap-2"}>
            {pairs && <p className="text-xs text-zinc-400">{group.entries.length > 1 ? `Pair ${g + 1}` : "Bye: plays without a vote"}</p>}
            {group.entries.map(entry => {
              const voted = ballot.my_votes.includes(entry.submission_id);
              return (
                <SongRow key={entry.submission_id} song={entry.song} note={entry.mine ? "your song" : undefined}>
                  {entry.votable && (
                    <button
                      onClick={() => onVote(entry.submission_id)}
                      disabled={busy || voted || left <= 0 || (pairs && votedHere)}
                      aria-label={voted ? `Voted for ${entry.song.title}` : `Vote for ${entry.song.title}`}
                      className={`min-h-11 min-w-11 px-4 rounded-lg font-semibold disabled:opacity-60 ${voted ? "bg-green-800" : "bg-[#5C172E] hover:bg-[#FF0048]"}`}
                    >
                      {voted ? "Voted" : "Vote"}
                    </button>
                  )}
                </SongRow>
              );
            })}
          </div>
        );
      })}
    </div>
  );
}

function RoundResults({ result, name }: { result: RoundResult; name: (id: string) => string }) {
  return (
    <details className="rounded-lg bg-zinc-950/50 p-3" open>
      <summary className="cursor-pointer min-h-11 flex items-center text-sm font-semibold">Round {result.round} results</summary>
      <ul className="flex flex-col gap-2 mt-2">
        {result.entries.map(e => (
          <li key={e.submission_id}>
            <SongRow song={e.song} note={`${name(e.submitted_by_user_id)} · ${e.votes} vote${e.votes === 1 ? "" : "s"}${e.points ? ` · +${e.points}` : ""}`}>
              {e.winner && <span className="px-2 py-1 rounded bg-[#FF0048] text-xs font-bold">Winner</span>}
            </SongRow>
          </li>
        ))}
      </ul>
    </details>
  );
}

function Reveal({ result, name, me }: { result: RoundResult; name: (id: string) => string; me: string | undefined }) {
  const song = result.entries[0];
  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-zinc-400">Round {result.round}: it was</p>
      <p className="text-3xl font-extrabold text-[#05EEFF]">{result.answer ?? song?.song.title}</p>
      {song && <SongRow song={song.song} note={`proposed by ${name(song.submitted_by_user_id)}${song.points ? ` · +${song.points}` : ""}`} />}
      {result.guesses.length === 0 ? <p className="text-zinc-300">Nobody named it.</p> : (
        <ol className="flex flex-col gap-1" aria-label="Correct guesses">
          {result.guesses.map(g => (
            <li key={g.user_id} className={`flex justify-between rounded-lg px-3 py-2 text-sm ${g.user_id === me ? "bg-[#5C172E]" : "bg-zinc-800"}`}>
              <span>{g.rank}. {name(g.user_id)}</span><span className="font-mono">+{g.points}</span>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}
