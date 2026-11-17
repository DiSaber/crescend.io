"use client";
// Thumbnails come from YouTube and prompt images are blob URLs: the static export has no image optimizer.
/* eslint-disable @next/next/no-img-element */

import { useEffect, useState, type ReactNode } from "react";
import { fetchImage, joinNames, leaders, pairSummary, resultGroups, type GameView, type Phase } from "../../lib/game";

/** Milliseconds until `iso` on the server's clock (`offset` = server minus device). */
export function remainingMs(iso: string | null, offset: number) {
  if (!iso) return null;
  const end = Date.parse(iso);
  return Number.isNaN(end) ? null : Math.max(0, end - (Date.now() + offset));
}

/** Re-renders its caller a few times a second while `active`. */
export function useTick(active: boolean) {
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => setTick(n => n + 1), 250);
    return () => clearInterval(timer);
  }, [active]);
}

/** Counts down to a server deadline; never trusts the device clock alone. */
export function Countdown({ endsAt, offset, size = "md", label }: {
  endsAt: string | null; offset: number; size?: "md" | "xl"; label?: string;
}) {
  useTick(!!endsAt);
  const ms = remainingMs(endsAt, offset);
  if (ms === null) return null;
  const s = Math.ceil(ms / 1000);
  const text = s >= 60 ? `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}` : `${s}`;
  const urgent = s <= 5;
  // Screen readers hear milestones, not every second.
  const milestone = MILESTONES.find(m => s <= m && (m === 0 || s > 0));
  return (
    <>
      <span
        role="timer"
        aria-label={`${label ?? "Time left"}: ${s} seconds`}
        className={`font-mono font-extrabold tabular-nums ${size === "xl" ? "text-7xl xl:text-8xl" : "text-3xl"} ${urgent ? "text-[#FF0048]" : "text-[#05EEFF]"}`}
      >
        {text}
      </span>
      <span className="sr-only" aria-live="polite" aria-atomic="true">
        {milestone === undefined ? "" : milestone === 0 ? "Time is up." : `${milestone} seconds left.`}
      </span>
    </>
  );
}

const MILESTONES = [0, 5, 10, 30];

/**
 * One polite live region per game view: announces each new phase, and the round's
 * answer or winners when results arrive, without re-reading the whole card.
 */
export function GameAnnouncer({ game, name }: { game: GameView; name: NameFn }) {
  const result = game.last_result;
  let text = `${roundLabel(game)}. ${phaseName(game)}.`;
  if (game.phase === "reveal" && result) {
    if (result.note === "nobody_to_guess") text += " Skipped: nobody left to guess.";
    text += ` It was ${result.answer ?? result.entries[0]?.song.title ?? "a mystery"}.`;
    if (result.note !== "nobody_to_guess") text += result.guesses.length ? ` ${result.guesses.length} named it; first was ${name(result.guesses[0].user_id, result.guesses[0].nickname)}.` : " Nobody named it.";
  } else if (result && game.mode.rules.ballot === "pairs" && game.phase !== "ended") {
    text += ` Round ${result.round}: ${resultGroups(result).map(pairSummary).join("; ")}.`;
  } else if (result && game.mode.rules.collect === "pick" && game.phase !== "ended") {
    const winners = result.entries.filter(e => e.winner);
    if (winners.length) text += ` Round ${result.round} winner: ${winners.map(w => `${w.song.title}, picked by ${name(w.submitted_by_user_id, w.submitted_by_nickname)}`).join("; ")}.`;
  }
  if (game.phase === "ended") {
    if (result?.note === "no_proposals") text += " Nobody proposed a song.";
    const top = leaders(game);
    const names = joinNames(top.map(t => name(t.user_id, t.nickname)));
    text += top.length === 0 ? " No points this time." : top.length > 1 ? ` Tie: ${names} share the win with ${top[0].points} points.` : ` ${names} wins with ${top[0].points} points.`;
  }
  return <p className="sr-only" role="status" aria-live="polite" aria-atomic="true">{text}</p>;
}

export const phaseTitle: Record<Phase, string> = {
  collect: "Pick a song", vote: "Vote", play: "Now playing", guess: "Name that tune", reveal: "The answer", ended: "Game over",
};

export function phaseName(game: GameView) {
  if (game.phase === "collect" && game.mode.rules.collect === "propose") return "Propose songs";
  return phaseTitle[game.phase];
}

/**
 * "Party · Round 2 of 5". During play with overlap the song playing may be an
 * earlier round's winner: the play heading names the round actually playing.
 */
export function roundLabel(game: GameView) {
  if (game.phase === "ended") return game.mode.name;
  if (game.mode.rules.collect === "propose" && game.phase === "collect") return `${game.mode.name} · setup`;
  const round = game.phase === "play" && game.now_playing ? game.now_playing.round : game.round;
  return `${game.mode.name} · Round ${round}${game.total_rounds ? ` of ${game.total_rounds}` : ""}`;
}

export type NameFn = (userId: string | null | undefined, nickname: string | null | undefined) => string;

/** A player's nickname in this lobby, marked "(you)" for the caller. Render as text only. */
export function playerName(userId: string | null | undefined, nickname: string | null | undefined, me: string | undefined) {
  if (!userId || !nickname) return "someone";
  return userId === me ? `${nickname} (you)` : nickname;
}

export function Scoreboard({ game, ownerId, me, large = false }: {
  game: GameView; ownerId: string | undefined; me: string | undefined; large?: boolean;
}) {
  const rows = game.scoreboard;
  // Shared points share a rank: 1, 2, 2, 4.
  const ranks = rows.map(row => rows.findIndex(r => r.points === row.points) + 1);
  // A TV board fits up to 20 players without scrolling: tighter rows, then two columns.
  const crowded = large && rows.length > 8;
  const row = large ? (crowded ? "py-1 text-lg" : "py-2 text-2xl") : "py-2 text-sm";
  return (
    <section aria-label="Scoreboard" className={`bg-zinc-900 rounded-xl border border-zinc-800 p-4 ${large ? "min-h-0 overflow-hidden" : ""}`}>
      <h3 className={`font-semibold mb-2 ${large ? "text-3xl" : ""}`}>Scoreboard</h3>
      {rows.length === 0 ? <p className="text-sm text-zinc-400">No players yet.</p> : (
        <ol className={large ? (crowded && rows.length > 12 ? "grid grid-cols-2 gap-1" : crowded ? "space-y-1" : "space-y-2") : "space-y-1"}>
          {rows.map((entry, i) => {
            const rank = ranks[i];
            return (
              <li key={entry.user_id} className={`flex items-center gap-3 rounded-lg px-3 ${row} ${entry.user_id === me ? "bg-[#5C172E]" : "bg-zinc-800"} ${entry.active ? "" : "text-zinc-400"}`}>
                <span className="w-6 text-zinc-400 tabular-nums">{rank}</span>
                <span className="flex-1 min-w-0 truncate">{playerName(entry.user_id, entry.nickname, me)}{entry.user_id === ownerId && <span className="text-zinc-400"> · host</span>}{!entry.active && " (left)"}</span>
                <span className="font-mono font-bold tabular-nums">{entry.points}<span className="sr-only"> points</span></span>
              </li>
            );
          })}
        </ol>
      )}
    </section>
  );
}

/** Game images need the bearer token, so they load through fetch into a blob URL. */
export function GameImage({ membershipId, imageId, alt, className }: {
  membershipId: string; imageId: string; alt: string; className?: string;
}) {
  const [loaded, setLoaded] = useState<{ id: string; url?: string; error?: string }>({ id: "" });
  useEffect(() => {
    const controller = new AbortController();
    let url: string | undefined;
    fetchImage(membershipId, imageId, controller.signal)
      .then(blob => { url = URL.createObjectURL(blob); setLoaded({ id: imageId, url }); })
      .catch(e => { if (!controller.signal.aborted) setLoaded({ id: imageId, error: e instanceof Error ? e.message : "Image unavailable" }); });
    return () => { controller.abort(); if (url) URL.revokeObjectURL(url); };
  }, [membershipId, imageId]);
  const current = loaded.id === imageId ? loaded : undefined;
  if (current?.url) return <img src={current.url} alt={alt} className={className} />;
  return (
    <div className={`${className ?? ""} flex items-center justify-center bg-zinc-800 text-sm text-zinc-400`} role={current?.error ? "alert" : "status"}>
      {current?.error ?? "Loading image…"}
    </div>
  );
}

export function SongRow({ song, children, note }: {
  song: { title: string; author_name: string; thumbnail_url: string }; note?: string; children?: ReactNode;
}) {
  return (
    <div className="flex flex-wrap sm:flex-nowrap items-center gap-x-3 gap-y-1 rounded-lg bg-zinc-800 p-2">
      <img src={song.thumbnail_url} alt="" className="h-12 w-20 shrink-0 rounded object-cover" />
      <div className="min-w-0 flex-1 basis-32">
        <p className="line-clamp-2 text-sm font-medium">{song.title}</p>
        <p className="line-clamp-1 text-xs text-zinc-400">{song.author_name}</p>
        {note && <p className="text-xs text-zinc-200">{note}</p>}
      </div>
      {children && <div className="basis-full sm:basis-auto flex justify-end gap-2">{children}</div>}
    </div>
  );
}
