"use client";

import { useEffect, useState, useSyncExternalStore } from "react";
import { useRouter } from "next/navigation";
import { QRCodeSVG } from "qrcode.react";
import Jukebox from "../components/jukebox.jsx";
import SignInGate from "../components/SignInGate";
import YouTubePlayer from "../components/YouTubePlayer";
import SongPicker from "../components/SongPicker";
import GamePanel from "../components/game/GamePanel";
import GameSetup from "../components/game/GameSetup";
import HostGamePlayer from "../components/game/HostGamePlayer";
import { youTubeVideoUrl } from "../lib/youtube";
import { account, logout, type Account } from "../lib/auth";
import { JOIN_NOTICE_KEY, removeMember, type Member } from "../lib/lobby";
import { QueueSession } from "../lib/queue-state";
import { type AdvanceReason, type QueueEntry, type QueueView } from "../lib/queue";

/**
 * Where the host's jukebox song was when a game took over the player, per queue
 * entry, so "Back to the jukebox" resumes it there. Session storage: a lost value
 * only means the song restarts.
 */
const positionKey = (entryId: string) => `crescend.jukebox.position.${entryId}`;
function savedPosition(entryId: string) {
  try { const v = Number(sessionStorage.getItem(positionKey(entryId))); return Number.isFinite(v) && v > 0 ? Math.floor(v) : 0; } catch { return 0; }
}
function savePosition(entryId: string, seconds: number) {
  try { sessionStorage.setItem(positionKey(entryId), String(seconds)); } catch { /* private mode: restart instead */ }
}
function forgetPosition(entryId: string) {
  try { sessionStorage.removeItem(positionKey(entryId)); } catch { /* nothing stored */ }
}

/** The host's queue player, starting where this entry was last paused. */
function JukeboxPlayer({ entry, disabled, onEnded, onSkipError }: {
  entry: QueueEntry; disabled: boolean; onEnded: () => void; onSkipError: () => void;
}) {
  // Fixed for this mount: the saved position only matters when the player comes back.
  const [start] = useState(() => savedPosition(entry.id));
  return (
    <YouTubePlayer
      videoId={entry.video.video_id}
      occurrence={entry.id}
      start={start}
      disabled={disabled}
      onPosition={seconds => savePosition(entry.id, seconds)}
      onEnded={() => { forgetPosition(entry.id); onEnded(); }}
      onSkipError={() => { forgetPosition(entry.id); onSkipError(); }}
    />
  );
}

function Lobby() {
  const router = useRouter();
  const [session] = useState(() => new QueueSession());
  const state = useSyncExternalStore(session.subscribe, session.getSnapshot, session.getSnapshot);
  const { membership, queue, userId, live, busy, ended, attempt } = state;
  const lobby = membership?.lobby;
  const [link, setLink] = useState("");
  const [setupOpen, setSetupOpen] = useState(false);
  const [me, setMe] = useState<Account | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);
  const [memberError, setMemberError] = useState<string | null>(null);
  useEffect(() => { session.start(); return () => session.stop(); }, [session]);
  useEffect(() => {
    let active = true;
    account().then((value) => { if (active) setMe(value ?? null); }).catch(() => {});
    // Set by the join page when the name came back with a suffix.
    try {
      const raw = sessionStorage.getItem(JOIN_NOTICE_KEY);
      sessionStorage.removeItem(JOIN_NOTICE_KEY);
      const parsed = raw ? JSON.parse(raw) : null;
      if (parsed && typeof parsed.wanted === "string" && typeof parsed.got === "string") {
        queueMicrotask(() => setNotice(`You're "${parsed.got}" because "${parsed.wanted}" is taken.`));
      }
    } catch { /* No notice. */ }
    return () => { active = false; };
  }, []);

  async function leave() {
    if (session.isHost()) {
      const people = (membership?.lobby?.members.length ?? 1) - 1;
      const others = people === 1 ? "the 1 other person" : `all ${people} other people`;
      const warning = state.game?.game?.status === "running"
        ? `A game is in progress. Ending the session stops the game for everyone and sends ${others} out. The code stops working. End the session for everyone?`
        : `End the session for everyone? This sends ${others} out and the code stops working.`;
      if (!window.confirm(warning)) return;
    }
    if (await session.leave()) router.push("/");
  }
  async function signOut() {
    if (session.isHost() && !window.confirm(
      "You're hosting this session. Signing out leaves it running for everyone, but you can't control it " +
      "(or play its music) from this browser until you sign in again. Sign out?",
    )) return;
    await logout().catch(() => {});
    router.push("/");
  }
  async function remove(member: Member) {
    const id = membership?.membership_id;
    if (!id || !window.confirm(
      `Remove ${member.nickname} from the session? Their skip votes stop counting and the game stops waiting for them. ` +
      "They keep their points, and they can rejoin with the code.",
    )) return;
    setRemoving(member.user_id);
    setMemberError(null);
    try {
      await removeMember(id, member.user_id);
      await session.sync();
    } catch (err) {
      setMemberError((err as Error).message);
    } finally {
      setRemoving(null);
    }
  }
  async function submitLink(text: string) {
    const url = youTubeVideoUrl(text);
    if (!url || session.getSnapshot().attempt || session.getSnapshot().busy) return;
    await session.add(url);
    if (!session.getSnapshot().attempt && !session.getSnapshot().error) setLink("");
  }
  const current = queue?.current ?? null;
  const upcoming = queue?.upcoming ?? [];
  const isHost = session.isHost();
  const threshold = queue?.voting.effective_threshold ?? null;
  const error = state.error ?? state.syncError;
  const adding = busy === "Adding music…";
  // Capture this render's membership epoch, never substitute a new generation in callbacks.
  const epoch = session.capture();
  const advanceFrom = (entry: QueueEntry, reason: AdvanceReason) => {
    void session.advanceEntry(entry.id, reason, epoch).catch(() => {});
  };
  const disabled = !!busy || !!state.syncError;
  const game = state.game?.game ?? null;
  // While a game runs the host player plays the game's songs; the queue keeps its place.
  const gameRunning = game?.status === "running";
  // After a game the jukebox stays paused behind the Game over card until the host
  // presses "Back to the jukebox" (which dismisses the game).
  const jukeboxPaused = !!game;

  if (!membership) return (
    <div className="text-center">
      <p role="status">{state.loading ? "Loading session…" : "Could not load the session."}</p>
      {error && <p role="alert" className="text-red-400">{error}</p>}
      {!state.loading && <button onClick={() => void session.sync()} className="min-h-11 px-4 underline">Retry</button>}
    </div>
  );
  if (!lobby) return (
    <div className="w-full max-w-md bg-zinc-900 p-8 rounded-xl border border-zinc-800 text-center">
      <p className="text-zinc-300 mb-6">{ended ?? "You're not in a session right now."}</p>
      <div className="flex flex-col gap-3">
        <a href="/join/" className="flex min-h-12 items-center justify-center p-3 rounded-lg bg-purple-600 hover:bg-purple-700 font-semibold">
          {ended ? "Join another session" : "Join with a code"}
        </a>
        {me && !me.guest && (
          <a href="/create/" className="flex min-h-12 items-center justify-center p-3 rounded-lg border border-zinc-600 hover:border-zinc-400 font-semibold text-zinc-200">
            Host a session
          </a>
        )}
      </div>
      <button onClick={() => void session.sync()} className="min-h-11 px-4 underline">Check session again</button>
      {error && <p role="alert" className="text-red-400">{error}</p>}
    </div>
  );

  // Nicknames are member-chosen text; JSX renders them as text, never markup.
  const contributor = (entry: QueueEntry) =>
    entry.submitted_by_user_id === userId ? "you" : entry.submitted_by_nickname;

  const voteLabel = (entry: QueueEntry) =>
    threshold === null ? `${entry.active_vote_count} skip votes` : `${entry.active_vote_count}/${threshold} to skip`;

  const entryControls = (entry: QueueEntry, index?: number) => {
    return (
      <div className="flex flex-wrap items-center justify-end gap-1">
        <span className="px-2 text-xs text-zinc-400" title="Skip votes from guests">
          {voteLabel(entry)}
        </span>
        {!isHost && !!userId && (
          <button
            type="button"
            onClick={() => session.vote(entry.id).catch(() => {})}
            disabled={entry.has_voted || disabled}
            className="min-h-11 rounded-lg px-3 text-sm text-zinc-200 bg-zinc-700 hover:bg-zinc-600 disabled:opacity-50"
          >
            {entry.has_voted ? "Voted" : "Vote skip"}
          </button>
        )}
        {isHost && index !== undefined && (
          <>
            <button
              type="button"
              onClick={() => session.move(entry.id, queue!.revision, index - 1).catch(() => {})}
              disabled={index === 0 || disabled}
              aria-label={`Move ${entry.video.title} up`}
              className="min-h-11 min-w-11 rounded-lg text-sm text-zinc-300 hover:bg-zinc-700 disabled:cursor-not-allowed disabled:opacity-30"
            >
              ↑
            </button>
            <button
              type="button"
              onClick={() => session.move(entry.id, queue!.revision, index + 1).catch(() => {})}
              disabled={index === upcoming.length - 1 || disabled}
              aria-label={`Move ${entry.video.title} down`}
              className="min-h-11 min-w-11 rounded-lg text-sm text-zinc-300 hover:bg-zinc-700 disabled:cursor-not-allowed disabled:opacity-30"
            >
              ↓
            </button>
          </>
        )}
        {isHost && (
          <button
            type="button"
            onClick={() => session.remove(entry.id).catch(() => {})}
            disabled={disabled}
            aria-label={`Veto ${entry.video.title}`}
            className="min-h-11 rounded-lg px-3 text-sm text-red-300 hover:bg-red-950 disabled:opacity-50"
          >
            Veto
          </button>
        )}
      </div>
    );
  };

  return (
    <div className="w-full flex flex-col items-center gap-8 py-10">
      <h1 className="sr-only">Session {lobby.join_code}</h1>
      {notice && (
        <div role="status" className="w-full max-w-4xl flex items-center justify-between gap-3 rounded-xl border border-amber-700 bg-amber-950/60 px-4 py-2 text-sm text-amber-100">
          <span className="min-w-0 break-words">{notice}</span>
          <button type="button" onClick={() => setNotice(null)} className="min-h-11 min-w-11 shrink-0 underline">OK</button>
        </div>
      )}
      <div className="flex flex-col md:flex-row gap-6 items-center md:items-start w-full max-w-4xl">
        <div className="bg-zinc-900 p-6 rounded-xl border border-zinc-800 text-center">
          <p className="text-zinc-400 text-sm">Session code</p>
          <p className="text-5xl font-mono font-extrabold tracking-[0.2em] text-[#05EEFF]">{lobby.join_code}</p>
          <p className={`mt-2 text-xs ${live ? "text-green-400" : "text-zinc-400"}`}>{live ? "● live" : "○ connecting"}</p>
          <a href="/lobby/tv/" target="_blank" rel="noopener" className="mt-1 inline-flex min-h-11 items-center text-sm text-zinc-300 underline hover:text-white">Board view for a TV ↗</a>
          {isHost && (
            <div className="mt-4 flex flex-col items-center gap-2">
              {/* White border is the quiet zone phone cameras need to read the code. */}
              <QRCodeSVG
                value={`${window.location.origin}/join/?code=${lobby.join_code}`}
                size={176}
                marginSize={2}
                title="Scan to join this session"
                className="rounded-lg"
              />
              <p className="text-xs text-zinc-400">Scan to join</p>
            </div>
          )}
        </div>

        <div className="flex-1 bg-zinc-900 p-6 rounded-xl border border-zinc-800">
          <p className="text-zinc-400 text-sm mb-2">In this session ({lobby.members.length})</p>
          <ul className="flex flex-wrap gap-2">
            {lobby.members.map((m) => (
              <li key={m.user_id} className={`flex max-w-full items-center gap-1 rounded-full text-sm ${isHost && m.role !== "owner" ? "pl-3 pr-1" : "px-3 py-1"} ${m.role === "owner" ? "bg-[#5C172E]" : "bg-zinc-800"}`}>
                <span className="min-w-0 break-words">
                  {m.nickname}
                  {m.role === "owner" && <span className="text-zinc-400"> · host</span>}
                  {m.user_id === userId && " (you)"}
                </span>
                {isHost && m.role !== "owner" && (
                  <button
                    type="button"
                    onClick={() => void remove(m)}
                    disabled={removing !== null}
                    aria-label={`Remove ${m.nickname}`}
                    className="min-h-11 shrink-0 rounded-full px-3 text-xs text-zinc-400 hover:bg-red-950 hover:text-red-300 disabled:opacity-50"
                  >
                    {removing === m.user_id ? "Removing…" : "Remove"}
                  </button>
                )}
              </li>
            ))}
          </ul>
          {memberError && <p role="alert" className="mt-2 text-sm text-red-400">{memberError}</p>}
          {isHost && queue && (
            <VotingPolicy
              key={`${queue.voting.mode}-${queue.voting.fixed_count}`}
              voting={queue.voting}
              disabled={disabled}
              onChange={(policy) => session.policy(policy).catch(() => {})}
            />
          )}
          <div className="mt-4 flex flex-wrap items-center gap-x-4 gap-y-1">
            <button onClick={leave} disabled={!!busy} className="min-h-11 text-sm text-zinc-400 hover:text-red-400 underline">
              {isHost ? "End session for everyone" : "Leave session"}
            </button>
            {me && !me.guest && (
              <span className="text-sm text-zinc-400">
                Signed in with Google ·{" "}
                <button type="button" onClick={() => void signOut()} className="min-h-11 underline hover:text-white">Sign out</button>
              </span>
            )}
          </div>
        </div>
      </div>

      {game && <GamePanel
        session={session}
        state={state}
        onNewGame={() => setSetupOpen(true)}
        player={isHost && gameRunning ? <HostGamePlayer session={session} state={state} /> : undefined}
      />}
      {isHost && (!gameRunning) && (setupOpen
        ? <GameSetup session={session} state={state} onClose={() => setSetupOpen(false)} />
        : !game && (
          <button onClick={() => setSetupOpen(true)} className="w-full max-w-4xl min-h-14 rounded-xl border border-[#5C172E] bg-zinc-900 hover:border-[#FF0048] p-4 text-left">
            <span className="block text-lg font-bold">Start a game</span>
            <span className="block text-sm text-zinc-400">Party, Speed Round, Duel and more. The queue waits and resumes after.</span>
          </button>
        ))}

      {jukeboxPaused && <p className="w-full max-w-4xl text-sm text-zinc-400">
        The jukebox is paused{gameRunning ? " during the game" : ""}{upcoming.length + (current ? 1 : 0) > 0 && ` with ${upcoming.length + (current ? 1 : 0)} song${upcoming.length + (current ? 1 : 0) === 1 ? "" : "s"} waiting`}.{" "}
        {gameRunning ? "It picks up where it left off when the game ends." : isHost ? "Press “Back to the jukebox” to pick up where it left off." : "It picks up when the host goes back to the jukebox."}
      </p>}
      {!gameRunning && <div className="w-full max-w-4xl">
        <SongPicker
          value={link}
          onChange={setLink}
          onSubmit={url => { void submitLink(url); }}
          addingUrl={adding ? attempt?.url : undefined}
          disabled={disabled || !!attempt || !queue}
          inputDisabled={!!attempt || !!busy}
        />
      </div>}
      {busy && <p role="status" className="text-zinc-400 text-sm">{busy}</p>}
      {error && <div role="alert" className="text-center">
        <p className="text-red-400">{error}</p>
        {state.syncError && <button onClick={() => void session.sync()} className="min-h-11 px-3 underline">Reconnect</button>}
      </div>}
      {attempt && !busy && <div className="text-center text-sm text-zinc-300">
        <p>The previous add may have completed. Retrying keeps the same submission ID.</p>
        <button onClick={() => void session.retrySubmission()} disabled={!!state.syncError} className="min-h-11 px-3 underline disabled:opacity-50">Retry submission</button>
        <button onClick={() => session.discardSubmission()} className="min-h-11 px-3 underline">Start a different submission</button>
      </div>}

      {jukeboxPaused ? null : current ? (
        <div className="w-full max-w-4xl bg-zinc-900 p-4 rounded-xl border border-zinc-800">
          <div className="flex flex-col sm:flex-row sm:justify-between sm:items-center gap-2 sm:gap-4 mb-3">
            <div className="min-w-0">
              <p className="font-semibold line-clamp-2">Now playing: {current.video.title}</p>
              <p className="text-xs text-zinc-400 line-clamp-2">
                {current.video.author_name} · added by {contributor(current)}
              </p>
            </div>
            <div className="flex flex-wrap items-center justify-end gap-1 sm:shrink-0">
              {isHost && (
                <button
                  onClick={() => advanceFrom(current, "skip")}
                  disabled={disabled}
                  className="min-h-11 px-3 text-sm underline text-zinc-400 hover:text-white disabled:opacity-50"
                >
                  Skip
                </button>
              )}
              {entryControls(current)}
            </div>
          </div>
          {isHost ? (
            <JukeboxPlayer
              key={current.id}
              entry={current}
              disabled={disabled}
              onEnded={() => advanceFrom(current, "ended")}
              onSkipError={() => advanceFrom(current, "playback_error_skip")}
            />
          ) : (
            <div className="flex items-center gap-4">
              <img src={current.video.thumbnail_url} alt="" className="h-24 w-40 rounded object-cover" />
              <p className="text-sm text-zinc-300">
                Sound plays on the host&apos;s device. Add songs and vote to skip from here.
              </p>
            </div>
          )}
        </div>
      ) : (
        <div className="w-full max-w-4xl rounded-xl border border-dashed border-zinc-700 bg-zinc-900/70 p-8 text-center">
          <p className="font-semibold text-zinc-200">{queue ? "Queue is empty" : "Loading queue…"}</p>
          <p className="mt-2 text-sm text-zinc-400">
            Search for a song or paste a YouTube link above to start playback
            {isHost ? " on this device." : " on the host's device."}
          </p>
        </div>
      )}

      {!gameRunning && upcoming.length > 0 && (
        <section className="w-full max-w-4xl bg-zinc-900 p-4 rounded-xl border border-zinc-800">
          <h2 className="font-semibold mb-3">Up next</h2>
          <ol className="space-y-2">
            {upcoming.map((entry, index) => (
              <li key={entry.id} className="flex flex-wrap sm:flex-nowrap items-center gap-x-3 gap-y-1 rounded-lg bg-zinc-800 p-2">
                <span className="w-6 text-center text-sm text-zinc-400">{index + 1}</span>
                <img src={entry.video.thumbnail_url} alt="" className="h-12 w-20 rounded object-cover" />
                <div className="min-w-0 flex-1 basis-32">
                  <p className="line-clamp-2 text-sm font-medium">{entry.video.title}</p>
                  <p className="line-clamp-2 text-xs text-zinc-400">
                    {entry.video.author_name} · added by {contributor(entry)}
                  </p>
                </div>
                <div className="basis-full sm:basis-auto">{entryControls(entry, index)}</div>
              </li>
            ))}
          </ol>
        </section>
      )}

      {!gameRunning && <Jukebox
        className="max-w-4xl"
        tiles={[...(current ? [current] : []), ...upcoming].map((entry) => entry.video)}
        spinning={!!current}
      />}
    </div>
  );
}

function VotingPolicy({
  voting,
  disabled,
  onChange,
}: {
  voting: QueueView["voting"];
  disabled: boolean;
  onChange: (policy: { mode: "majority" } | { mode: "fixed"; count: number }) => void;
}) {
  const [mode, setMode] = useState(voting.mode);
  const [count, setCount] = useState(String(voting.fixed_count ?? 2));
  const valid = /^\d+$/.test(count) && Number(count) >= 1 && Number(count) <= 2147483647;
  return (
    <form onSubmit={e => { e.preventDefault(); if (mode === "majority" || valid) onChange(mode === "majority" ? { mode } : { mode, count: Number(count) }); }} className="mt-4 flex flex-wrap items-center gap-2 text-sm text-zinc-300">
      <label htmlFor="vote-mode">Vote skip needs</label>
      <select id="vote-mode" value={mode} disabled={disabled} onChange={e => setMode(e.target.value as "majority" | "fixed")} className="min-h-11 rounded bg-zinc-800 border border-zinc-700 px-2">
        <option value="majority">a majority of guests</option>
        <option value="fixed">a fixed number of votes</option>
      </select>
      {mode === "fixed" && <input aria-label="Required skip votes" type="number" min={1} max={2147483647} step={1} value={count} disabled={disabled} onChange={e => setCount(e.target.value)} className="min-h-11 w-24 rounded bg-zinc-800 border border-zinc-700 px-2" />}
      <button type="submit" disabled={disabled || (mode === "fixed" && !valid)} className="min-h-11 px-3 underline disabled:opacity-50">Apply policy</button>
      <span className="text-zinc-400">{voting.active_guest_count} guests; {voting.effective_threshold === null ? "no removals without guests" : `${voting.effective_threshold} votes needed`}</span>
    </form>
  );
}

export default function LobbyPage() {
  return (
    <main className="relative w-full min-h-screen flex flex-col items-center justify-center bg-gradient-to-b from-black to-[#1a0b29] text-white px-6">
      <SignInGate returnTo="/lobby/" joinFirst message="Join a session with its code. Hosting one needs Google.">
        <Lobby />
      </SignInGate>
    </main>
  );
}
