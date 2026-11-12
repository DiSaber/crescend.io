"use client";
// Thumbnails come from YouTube and prompt images are blob URLs: the static export has no image optimizer.
/* eslint-disable @next/next/no-img-element */

import { useEffect, useLayoutEffect, useState, useSyncExternalStore } from "react";
import { QRCodeSVG } from "qrcode.react";
import SignInGate from "../../components/SignInGate";
import { Countdown, GameAnnouncer, GameImage, Scoreboard, phaseName, playerName, roundLabel, type NameFn } from "../../components/game/bits";
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
  const name = (id: string | null | undefined, nickname: string | null | undefined) => playerName(id, nickname, undefined);
  const current = state.queue?.current;

  return (
    <div className="w-full max-w-[120rem] flex flex-col gap-6 py-6 lg:h-full lg:min-h-0">
      <header className="flex flex-wrap items-center justify-between gap-4 shrink-0">
        <div className="flex items-center gap-4">
          <QRCodeSVG value={`${window.location.origin}/join/?code=${lobby.join_code}`} size={192} marginSize={2} title="Scan to join this session" className="rounded-lg shrink-0 size-24" />
          <div>
            <p className="text-sm text-zinc-400">Join at {window.location.host} with code</p>
            <p className="text-4xl sm:text-6xl font-mono font-extrabold tracking-[0.2em] text-[#05EEFF]">{lobby.join_code}</p>
          </div>
        </div>
        <div className="text-right">
          <p className={`text-sm ${state.live ? "text-green-400" : "text-zinc-400"}`}>{state.live ? "● live" : "○ connecting"}</p>
          <a href="/lobby/" className="inline-flex min-h-11 items-center text-sm text-zinc-400 underline hover:text-white">Back to the lobby</a>
        </div>
      </header>

      {game ? (
        // On a TV (lg and up) the board fills the screen exactly: the grid takes the
        // height left under the header and the prompt image takes what the text leaves.
        <div className="grid gap-6 lg:grid-cols-[2fr_1fr] lg:flex-1 lg:min-h-0 lg:grid-rows-[minmax(0,1fr)]">
          <section aria-label="Game board" className="bg-zinc-900 rounded-xl border border-[#5C172E] p-6 flex flex-col gap-5 min-w-0 lg:min-h-0 lg:overflow-hidden">
            <GameAnnouncer game={game} name={name} />
            <div className="flex flex-wrap items-start justify-between gap-4 shrink-0">
              <div className="min-w-0">
                <p className="text-lg uppercase tracking-wider text-zinc-400">{roundLabel(game)}</p>
                <h1 className="text-5xl xl:text-6xl font-extrabold">{phaseName(game)}</h1>
              </div>
              <Countdown size="xl" endsAt={game.phase_ends_at ?? (game.phase === "play" ? game.now_playing?.ends_at ?? null : null)} offset={state.clockOffset} />
            </div>
            {game.prompt && (
              // While voting the ballot needs the room: the image sits beside the prompt
              // text instead of under it. Otherwise it takes the height left over.
              <div className={game.phase === "vote"
                ? "flex flex-row items-stretch gap-4 shrink-0"
                : `flex flex-col gap-4 min-h-0 lg:overflow-hidden ${game.prompt.image_id ? "lg:flex-1" : "shrink-0"}`}>
                {game.prompt.text && <p className={`font-bold line-clamp-3 shrink-0 ${game.phase === "vote" ? "flex-1 min-w-0 text-3xl xl:text-4xl" : "text-4xl xl:text-5xl"}`}>{game.prompt.text}</p>}
                {game.prompt.image_id && (
                  <div className={`relative rounded-xl bg-black overflow-hidden ${game.phase === "vote" ? "w-1/3 min-h-28 shrink-0" : "min-h-48 h-[50vh] lg:min-h-24 lg:h-auto lg:flex-1"}`}>
                    <GameImage membershipId={membershipId} imageId={game.prompt.image_id} alt={game.prompt.text ?? "Round image"} className="absolute inset-0 h-full w-full object-contain" />
                  </div>
                )}
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

function PhaseBoard({ game, name }: { game: GameView; name: NameFn }) {
  const big = "text-3xl xl:text-4xl";
  const entries = game.ballot?.groups.flatMap(g => g.entries) ?? [];
  // Twenty songs still fit on one screen next to the prompt: smaller cards, more columns.
  const dense = entries.length > 6;
  const packed = entries.length > 12;
  const playing = game.phase !== "guess" && game.now_playing && !game.now_playing.hidden ? game.now_playing.song : null;
  const winners = game.last_result?.entries.filter(e => e.winner) ?? [];
  return (
    <div className="flex flex-col gap-5 shrink-0 min-h-0">
      {game.phase === "collect" && (
        <p className={big}>
          {game.mode.rules.collect === "propose" ? "Secret songs proposed" : "Songs picked"}: <strong className="text-[#05EEFF]">{game.submission_count}</strong>
        </p>
      )}
      {game.phase === "vote" && game.ballot && (
        <ul className={`grid sm:grid-cols-2 ${packed ? "gap-1.5 lg:grid-cols-4" : dense ? "gap-2 lg:grid-cols-3" : "gap-4"}`} aria-label="Songs on the ballot">
          {entries.map(e => (
            <li key={e.submission_id} className={`flex items-center rounded-lg bg-zinc-800 min-w-0 ${packed ? "gap-2 p-1.5" : dense ? "gap-3 p-2" : "gap-4 p-3"}`}>
              <img src={e.song.thumbnail_url} alt="" className={`rounded object-cover shrink-0 ${packed ? "h-8 w-14" : dense ? "h-10 w-16" : "h-20 w-32"}`} />
              <p className={`font-semibold min-w-0 ${packed ? "text-base line-clamp-1" : dense ? "text-lg line-clamp-1" : "text-2xl line-clamp-2"}`}>{e.song.title}</p>
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
          {game.last_result.entries[0] && <p className="text-2xl text-zinc-300">Proposed by {name(game.last_result.entries[0].submitted_by_user_id, game.last_result.entries[0].submitted_by_nickname)} · +{game.last_result.entries[0].points}</p>}
          <ol className="flex flex-col gap-2">
            {game.last_result.guesses.length === 0 && <li className="text-2xl">Nobody named it.</li>}
            {game.last_result.guesses.map(g => <li key={g.user_id} className="text-2xl">{g.rank}. {name(g.user_id, g.nickname)} <span className="font-mono">+{g.points}</span></li>)}
          </ol>
        </div>
      )}
      {playing && (
        <div className="flex items-center gap-4">
          <img src={playing.thumbnail_url} alt="" className="h-24 w-40 rounded-lg object-cover shrink-0" />
          <div className="min-w-0">
            <p className="text-lg text-zinc-400">Now playing · picked by {name(game.now_playing?.submitted_by_user_id, game.now_playing?.submitted_by_nickname)}</p>
            <p className="text-3xl font-bold line-clamp-2">{playing.title}</p>
          </div>
        </div>
      )}
      {game.phase !== "reveal" && game.phase !== "guess" && game.mode.rules.collect === "pick" && winners.length > 0 && (
        <p className="text-2xl text-zinc-300">Round {game.last_result!.round} winner{winners.length > 1 ? "s" : ""}: {winners.map(w => `${w.song.title} (${name(w.submitted_by_user_id, w.submitted_by_nickname)})`).join(", ")}</p>
      )}
      {game.phase === "ended" && game.scoreboard[0] && (
        <p className="text-5xl font-extrabold">{game.scoreboard[0].points > 0 ? `${name(game.scoreboard[0].user_id, game.scoreboard[0].nickname)} wins!` : "Game over"}</p>
      )}
    </div>
  );
}

/**
 * TVs get the same layout at every resolution: the root font scales with the
 * viewport (16 px at 1280x720, 24 px at 1080p, 48 px at 4K), so rem-based type,
 * spacing and the QR code grow together instead of leaving a small island on 4K.
 */
function useTvScale() {
  useLayoutEffect(() => {
    const root = document.documentElement;
    const before = root.style.fontSize;
    root.style.fontSize = "clamp(16px, min(1.25vw, 2.2222vh), 64px)";
    return () => { root.style.fontSize = before; };
  }, []);
}

export default function BoardPage() {
  useTvScale();
  return (
    <main className="relative w-full min-h-screen lg:h-screen flex flex-col items-center justify-center bg-gradient-to-b from-black to-[#1a0b29] text-white px-4 sm:px-8">
      <SignInGate returnTo="/lobby/tv/" joinFirst message="Join a session with its code to show its board here. Hosting one needs Google.">
        <Board />
      </SignInGate>
    </main>
  );
}
