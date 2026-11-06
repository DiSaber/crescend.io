"use client";

import { useState } from "react";
import YouTubePlayer from "../YouTubePlayer";
import { advanceGame, type NowPlaying } from "../../lib/game";
import type { QueueSession, SessionSnapshot } from "../../lib/queue-state";

/**
 * The host device is the game's DJ: it plays `now_playing` in an ordinary visible
 * player (docs/game-modes.md, hidden-title policy) and reports each song's end with
 * its `play_seq`, so a late callback can never skip the next song.
 */
export default function HostGamePlayer({ session, state }: { session: QueueSession; state: SessionSnapshot }) {
  const game = state.game?.game;
  const now = game?.now_playing;
  if (!game || !now?.song) {
    return (
      <div className="w-full max-w-4xl rounded-xl border border-dashed border-zinc-700 bg-zinc-900/70 p-4 text-center text-sm text-zinc-400">
        The game&apos;s songs play here on your device. Nothing is playing yet.
      </div>
    );
  }
  return (
    <div className="w-full max-w-4xl bg-zinc-900 p-4 rounded-xl border border-zinc-800">
      <Occurrence key={`${game.id}-${now.play_seq}`} session={session} now={now} secret={game.phase === "guess"} offset={state.clockOffset} />
    </div>
  );
}

function Occurrence({ session, now, secret, offset }: { session: QueueSession; now: NowPlaying; secret: boolean; offset: number }) {
  // Fixed per occurrence: rejoin mid-song where the room already is, not from the top.
  const [start] = useState(() => {
    const started = Date.parse(now.started_at);
    const elapsed = Number.isNaN(started) ? 0 : Math.floor((Date.now() + offset - started) / 1000);
    return now.start_seconds + (elapsed > 2 ? elapsed : 0);
  });
  const [epoch] = useState(() => session.capture());
  const ended = () => { void session.gameMutate("Next song…", m => advanceGame(m, { play_seq: now.play_seq }), epoch).catch(() => {}); };
  return (
    <>
      <p className="font-semibold line-clamp-2 mb-1">{secret ? "Playing the mystery song" : `Now playing: ${now.song!.title}`}</p>
      <p className="text-xs text-zinc-400 mb-3">Game song{now.ends_at ? " · clip" : " · full song"}</p>
      <YouTubePlayer
        videoId={now.song!.video_id}
        occurrence={String(now.play_seq)}
        start={start}
        disabled={false}
        onEnded={ended}
        onSkipError={ended}
      />
    </>
  );
}
