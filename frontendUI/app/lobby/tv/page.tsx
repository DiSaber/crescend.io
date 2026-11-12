"use client";
// Thumbnails come from YouTube and prompt images are blob URLs: the static export has no image optimizer.
/* eslint-disable @next/next/no-img-element */

import { useEffect, useState, useSyncExternalStore } from "react";
import { QRCodeSVG } from "qrcode.react";
import SignInGate from "../../components/SignInGate";
import { Countdown, GameImage, Scoreboard, phaseName, playerName, roundLabel } from "../../components/game/bits";
import { QueueSession } from "../../lib/queue-state";
import type { GameView } from "../../lib/game";

/**
 * The shared board for a TV or projector (#48): prompt, timer, progress and
 * scoreboard, never a player. During Name That Tune's guess phase it shows no song
 * details at all, even when the host's account opens it, so the room can watch the
 * board while the host device plays the clip out of sight.
 */
function Board() {
  const [session] = useState(() => new QueueSession());
  const state = useSyncExternalStore(session.subscribe, session.getSnapshot, session.getSnapshot);
  useEffect(() => { session.start(); return () => session.stop(); }, [session]);
  const lobby = state.membership?.lobby;
  const membershipId = state.membership?.membership_id ?? undefined;
  const game = state.game?.game ?? null;

  if (!state.membership) return <p role="status" className="text-2xl text-zinc-400">{state.loading ? "Loading the board…" : "Could not load the session."}</p>;
  if (!lobby || !membershipId) return (
    <div className="text-center">
      <p className="text-2xl text-zinc-300 mb-4">{state.ended ?? "Join a session to show its board."}</p>
      <a href="/join/" className="inline-flex min-h-11 items-center px-6 rounded-lg bg-purple-600 font-semibold">Join a session</a>
    </div>
  );
  const ownerId = lobby.owner_user_id;
  const name = (id: string | null | undefined) => playerName(id, ownerId, undefined);
  const current = state.queue?.current;

  return (
    <div className="w-full max-w-7xl flex flex-col gap-6 py-6">
      <header className="flex flex-wrap items-center justify-between gap-4">
        <div className="flex items-center gap-4">
          <QRCodeSVG value={`${window.location.origin}/join/?code=${lobby.join_code}`} size={96} marginSize={2} title="Scan to join this session" className="rounded-lg shrink-0" />
          <div>
            <p className="text-sm text-zinc-400">Join at {window.location.host} with code</p>
            <p className="text-4xl sm:text-6xl font-mono font-extrabold tracking-[0.2em] text-[#05EEFF]">{lobby.join_code}</p>
          </div>
        </div>
        <div className="text-right">
          <p className={`text-sm ${state.live ? "text-green-400" : "text-zinc-500"}`}>{state.live ? "● live" : "○ connecting"}</p>
          <a href="/lobby/" className="inline-flex min-h-11 items-center text-sm text-zinc-400 underline hover:text-white">Back to the lobby</a>
        </div>
      </header>

      {game ? (
        <div className="grid gap-6 lg:grid-cols-[2fr_1fr]">
          <section aria-label="Game board" className="bg-zinc-900 rounded-xl border border-[#5C172E] p-6 flex flex-col gap-6 min-w-0">
            <div className="flex flex-wrap items-start justify-between gap-4">
              <div className="min-w-0">
                <p className="text-lg uppercase tracking-wider text-zinc-400">{roundLabel(game)}</p>
                <h1 className="text-5xl sm:text-6xl font-extrabold">{phaseName(game)}</h1>
              </div>
              <Countdown size="xl" endsAt={game.phase_ends_at ?? (game.phase === "play" ? game.now_playing?.ends_at ?? null : null)} offset={state.clockOffset} />
            </div>
            {game.prompt && (
              <div className="flex flex-col gap-4">
                {game.prompt.image_id && (
                  <GameImage membershipId={membershipId} imageId={game.prompt.image_id} alt={game.prompt.text ?? "Round image"} className="w-full max-h-[60vh] min-h-60 rounded-xl object-contain bg-black" />
                )}
                {game.prompt.text && <p className="text-4xl sm:text-5xl font-bold">{game.prompt.text}</p>}
              </div>
            )}
            <PhaseBoard game={game} name={name} />
          </section>
          <Scoreboard game={game} ownerId={ownerId} me={undefined} large />
        </div>
      ) : (
        <section aria-label="Jukebox board" className="bg-zinc-900 rounded-xl border border-zinc-800 p-6 flex flex-col gap-4">
          <h1 className="text-5xl font-extrabold">Jukebox</h1>
          {current ? (
            <div className="flex flex-wrap items-center gap-6">
              <img src={current.video.thumbnail_url} alt="" className="w-full max-w-md rounded-xl object-cover" />
              <div className="min-w-0">
                <p className="text-xl text-zinc-400">Now playing</p>
                <p className="text-4xl font-bold">{current.video.title}</p>
                <p className="text-xl text-zinc-400 mt-2">{state.queue?.upcoming.length ?? 0} up next</p>
              </div>
            </div>
          ) : <p className="text-2xl text-zinc-400">Scan the code to add a song.</p>}
        </section>
      )}
    </div>
  );
}

function PhaseBoard({ game, name }: { game: GameView; name: (id: string | null | undefined) => string }) {
  const big = "text-3xl sm:text-4xl";
  const playing = game.phase !== "guess" && game.now_playing && !game.now_playing.hidden ? game.now_playing.song : null;
  const winners = game.last_result?.entries.filter(e => e.winner) ?? [];
  return (
    <div className="flex flex-col gap-6">
      {game.phase === "collect" && (
        <p className={big}>
          {game.mode.rules.collect === "propose" ? "Secret songs proposed" : "Songs picked"}: <strong className="text-[#05EEFF]">{game.submission_count}</strong>
        </p>
      )}
      {game.phase === "vote" && game.ballot && (
        <ul className="grid gap-4 sm:grid-cols-2">
          {game.ballot.groups.flatMap(g => g.entries).map(e => (
            <li key={e.submission_id} className="flex items-center gap-4 rounded-xl bg-zinc-800 p-3">
              <img src={e.song.thumbnail_url} alt="" className="h-20 w-32 rounded object-cover shrink-0" />
              <p className="text-2xl font-semibold line-clamp-2">{e.song.title}</p>
            </li>
          ))}
        </ul>
      )}
      {game.phase === "guess" && (
        <div>
          <p className="text-5xl font-extrabold">♪ What&apos;s this song?</p>
          <p className={`${big} mt-4`}>
            <strong className="text-[#05EEFF]">{game.guess?.correct_count ?? 0}</strong> player{game.guess?.correct_count === 1 ? " has" : "s have"} named it
          </p>
          <p className="text-xl text-zinc-400 mt-2">Type your guess on your phone.</p>
        </div>
      )}
      {game.phase === "reveal" && game.last_result && (
        <div className="flex flex-col gap-4">
          <p className="text-5xl sm:text-6xl font-extrabold text-[#05EEFF]">{game.last_result.answer ?? game.last_result.entries[0]?.song.title}</p>
          {game.last_result.entries[0] && <p className="text-2xl text-zinc-300">Proposed by {name(game.last_result.entries[0].submitted_by_user_id)} · +{game.last_result.entries[0].points}</p>}
          <ol className="flex flex-col gap-2">
            {game.last_result.guesses.length === 0 && <li className="text-2xl">Nobody named it.</li>}
            {game.last_result.guesses.map(g => <li key={g.user_id} className="text-2xl">{g.rank}. {name(g.user_id)} <span className="font-mono">+{g.points}</span></li>)}
          </ol>
        </div>
      )}
      {playing && (
        <div className="flex items-center gap-4">
          <img src={playing.thumbnail_url} alt="" className="h-24 w-40 rounded-lg object-cover shrink-0" />
          <div className="min-w-0">
            <p className="text-lg text-zinc-400">Now playing · picked by {name(game.now_playing?.submitted_by_user_id)}</p>
            <p className="text-3xl font-bold line-clamp-2">{playing.title}</p>
          </div>
        </div>
      )}
      {game.phase !== "reveal" && game.phase !== "guess" && game.mode.rules.collect === "pick" && winners.length > 0 && (
        <p className="text-2xl text-zinc-300">Round {game.last_result!.round} winner{winners.length > 1 ? "s" : ""}: {winners.map(w => `${w.song.title} (${name(w.submitted_by_user_id)})`).join(", ")}</p>
      )}
      {game.phase === "ended" && game.scoreboard[0] && (
        <p className="text-5xl font-extrabold">{game.scoreboard[0].points > 0 ? `${name(game.scoreboard[0].user_id)} wins!` : "Game over"}</p>
      )}
    </div>
  );
}

export default function BoardPage() {
  return (
    <div className="relative w-full min-h-screen flex flex-col items-center justify-center bg-gradient-to-b from-black to-[#1a0b29] text-white px-4 sm:px-8">
      <SignInGate returnTo="/lobby/tv/">
        <Board />
      </SignInGate>
    </div>
  );
}
