"use client";

import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { useRouter } from "next/navigation";
import Jukebox from "../components/jukebox.jsx";
import SignInGate from "../components/SignInGate";
import {
  fetchMetadata,
  getCurrent,
  leaveLobby,
  streamLobbyEvents,
  type Lobby as LobbyState,
  type VideoMetadata,
} from "../lib/lobby";

type YouTubePlayerInstance = {
  destroy: () => void;
};

type YouTubeApi = {
  Player: new (
    element: HTMLDivElement,
    options: {
      videoId: string;
      playerVars?: Record<string, number | string>;
      events?: {
        onStateChange?: (event: { data: number }) => void;
        onError?: (event: { data: number }) => void;
      };
    },
  ) => YouTubePlayerInstance;
  PlayerState: { ENDED: number };
};

declare global {
  interface Window {
    YT?: YouTubeApi;
    onYouTubeIframeAPIReady?: () => void;
  }
}

let youtubeApiPromise: Promise<YouTubeApi> | undefined;

function loadYouTubeApi(): Promise<YouTubeApi> {
  if (youtubeApiPromise) return youtubeApiPromise;

  youtubeApiPromise = new Promise((resolve, reject) => {
    const existing = document.querySelector(
      'script[src="https://www.youtube.com/iframe_api"]',
    );
    const previousReady = window.onYouTubeIframeAPIReady;

    window.onYouTubeIframeAPIReady = () => {
      previousReady?.();
      if (window.YT) resolve(window.YT);
      else reject(new Error("YouTube player API did not initialize."));
    };

    if (window.YT?.Player) {
      resolve(window.YT);
      return;
    }

    if (existing) return;

    const script = document.createElement("script");
    script.src = "https://www.youtube.com/iframe_api";
    script.async = true;
    script.onerror = () => reject(new Error("Could not load the YouTube player."));
    document.head.appendChild(script);
  });

  return youtubeApiPromise;
}

function YouTubePlayer({
  video,
  onEnded,
  onError,
}: {
  video: VideoMetadata;
  onEnded: () => void;
  onError: (message: string) => void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const ended = useRef(false);

  useEffect(() => {
    let player: YouTubePlayerInstance | undefined;
    let cancelled = false;
    ended.current = false;

    loadYouTubeApi()
      .then((youtube) => {
        if (cancelled || !container.current) return;
        player = new youtube.Player(container.current, {
          videoId: video.video_id,
          playerVars: {
            autoplay: 1,
            enablejsapi: 1,
            origin: window.location.origin,
          },
          events: {
            onStateChange: (event) => {
              if (event.data === youtube.PlayerState.ENDED && !ended.current) {
                ended.current = true;
                onEnded();
              }
            },
            onError: () => onError("YouTube could not play this video."),
          },
        });
      })
      .catch(() => {
        if (!cancelled) onError("Could not load the YouTube player.");
      });

    return () => {
      cancelled = true;
      player?.destroy();
    };
  }, [video.video_id, onEnded, onError]);

  return (
    <div className="relative w-full aspect-video rounded-lg overflow-hidden bg-black">
      <div ref={container} className="absolute inset-0" />
      <noscript>JavaScript is required to play YouTube videos.</noscript>
    </div>
  );
}

const ENDED_MESSAGES = {
  left: "You left the session.",
  closed: "The host closed the session.",
  expired: "The session expired.",
};

function Lobby() {
  const router = useRouter();
  const [membershipId, setMembershipId] = useState<string | null>();
  const [lobby, setLobby] = useState<LobbyState | null>(null);
  const [ended, setEnded] = useState<string>();
  const [error, setError] = useState<string>();
  const [live, setLive] = useState(false);

  // Queue is local to this browser until the backend has queue endpoints.
  const [queue, setQueue] = useState<VideoMetadata[]>([]);
  const [link, setLink] = useState("");
  const [adding, setAdding] = useState(false);

  // Serialized refetch: a hint while a read is in flight triggers one more read after it.
  const inFlight = useRef(false);
  const dirty = useRef(false);
  const sync = useCallback(async () => {
    dirty.current = true;
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      while (dirty.current) {
        dirty.current = false;
        const current = await getCurrent();
        setMembershipId(current.membership_id);
        setLobby(current.lobby);
      }
      setError(undefined);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      inFlight.current = false;
    }
  }, []);

  useEffect(() => {
    sync();
  }, [sync]);

  // Live updates for this membership generation, reconnecting with backoff.
  useEffect(() => {
    if (!membershipId) return;
    const controller = new AbortController();
    let delay = 250 + Math.random() * 250;
    let stop = false;
    (async () => {
      while (!stop && !controller.signal.aborted) {
        try {
          await streamLobbyEvents(membershipId, controller.signal, (e) => {
            setLive(true);
            delay = 250;
            if (e.event === "sync_required" || e.event === "lobby_changed") sync();
            if (e.event === "membership_ended") {
              stop = true;
              setEnded(ENDED_MESSAGES[e.data.reason]);
              setMembershipId(null);
              setLobby(null);
            }
            // auth_expired: the stream closes; the next connect refreshes the token.
          });
        } catch {
          if (controller.signal.aborted) return;
          sync(); // a 403 means the generation changed; current state tells us the new one
        }
        setLive(false);
        if (stop) return;
        await new Promise((r) => setTimeout(r, delay));
        delay = Math.min(delay * 2, 10_000) * (0.75 + Math.random() * 0.5);
      }
    })();
    return () => controller.abort();
  }, [membershipId, sync]);

  async function leave() {
    if (!membershipId) return;
    try {
      await leaveLobby(membershipId);
      router.push("/");
    } catch (e) {
      setError((e as Error).message);
    }
  }

  async function addSong(e: FormEvent) {
    e.preventDefault();
    setAdding(true);
    try {
      const video = await fetchMetadata(link.trim());
      setQueue((q) => [...q, video]);
      setLink("");
      setError(undefined);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setAdding(false);
    }
  }

  const advancePlayback = useCallback(() => {
    setQueue((current) => current.slice(1));
  }, []);

  const removeFromQueue = useCallback((index: number) => {
    setQueue((current) => current.filter((_, itemIndex) => itemIndex !== index));
  }, []);

  const moveInQueue = useCallback((index: number, direction: -1 | 1) => {
    setQueue((current) => {
      const targetIndex = index + direction;
      if (index < 1 || targetIndex < 1 || targetIndex >= current.length) {
        return current;
      }

      const next = [...current];
      [next[index], next[targetIndex]] = [next[targetIndex], next[index]];
      return next;
    });
  }, []);

  const reportPlayerError = useCallback((message: string) => {
    setError(message);
  }, []);

  if (membershipId === undefined) return <p className="text-zinc-400">Loading session…</p>;

  if (!lobby) {
    return (
      <div className="w-full max-w-md bg-zinc-900 p-8 rounded-xl border border-zinc-800 text-center">
        <p className="text-zinc-300 mb-6">{ended ?? "You're not in a session right now."}</p>
        <div className="flex gap-4">
          <a href="/create/" className="flex-1 p-3 rounded-lg bg-purple-600 hover:bg-purple-700 font-semibold">Create</a>
          <a href="/join/" className="flex-1 p-3 rounded-lg bg-purple-600 hover:bg-purple-700 font-semibold">Join</a>
        </div>
      </div>
    );
  }

  const nowPlaying = queue[0];

  return (
    <div className="w-full flex flex-col items-center gap-8 py-10">
      <div className="flex flex-col md:flex-row gap-6 items-center md:items-start w-full max-w-4xl">
        <div className="bg-zinc-900 p-6 rounded-xl border border-zinc-800 text-center">
          <p className="text-zinc-400 text-sm">Session code</p>
          <p className="text-5xl font-mono font-extrabold tracking-[0.2em] text-[#05EEFF]">{lobby.join_code}</p>
          <p className={`mt-2 text-xs ${live ? "text-green-400" : "text-zinc-500"}`}>{live ? "● live" : "○ connecting"}</p>
        </div>

        <div className="flex-1 bg-zinc-900 p-6 rounded-xl border border-zinc-800">
          <p className="text-zinc-400 text-sm mb-2">In this session ({lobby.members.length})</p>
          <ul className="flex flex-wrap gap-2">
            {lobby.members.map((m) => (
              <li key={m.user_id} className={`px-3 py-1 rounded-full text-sm ${m.role === "owner" ? "bg-[#5C172E]" : "bg-zinc-800"}`}>
                {m.role === "owner" ? "Host" : "Guest"} #{m.user_id}
              </li>
            ))}
          </ul>
          <button onClick={leave} className="mt-4 text-sm text-zinc-400 hover:text-red-400 underline">
            Leave session (closes it for everyone if you&apos;re the host)
          </button>
        </div>
      </div>

      <form onSubmit={addSong} className="flex gap-3 w-full max-w-4xl">
        <input
          value={link}
          onChange={(e) => setLink(e.target.value)}
          placeholder="Paste a YouTube link"
          className="flex-1 p-3 rounded-lg bg-zinc-800 border border-zinc-700 placeholder-zinc-500 focus:outline-none focus:border-purple-500"
        />
        <button
          type="submit"
          disabled={adding || !link.trim()}
          className="px-6 rounded-lg bg-[#5C172E] hover:bg-[#FF0048] disabled:opacity-50 transition font-bold"
        >
          {adding ? "Adding…" : "Add music"}
        </button>
      </form>
      {error && <p className="text-red-400">{error}</p>}

      {nowPlaying ? (
        <div className="w-full max-w-4xl bg-zinc-900 p-4 rounded-xl border border-zinc-800">
          <div className="flex justify-between items-center mb-3">
            <p className="font-semibold truncate">Now playing: {nowPlaying.title}</p>
            <button onClick={advancePlayback} className="text-sm underline text-zinc-400 hover:text-white shrink-0 ml-4">
              Skip
            </button>
          </div>
          <YouTubePlayer
            key={nowPlaying.video_id}
            video={nowPlaying}
            onEnded={advancePlayback}
            onError={reportPlayerError}
          />
          <p className="mt-2 text-xs text-zinc-500">
            Playback and the queue are only on this device for now; shared
            playback needs backend support.
          </p>
        </div>
      ) : (
        <div className="w-full max-w-4xl rounded-xl border border-dashed border-zinc-700 bg-zinc-900/70 p-8 text-center">
          <p className="font-semibold text-zinc-200">Queue is empty</p>
          <p className="mt-2 text-sm text-zinc-500">
            Add a YouTube link above to start playback.
          </p>
        </div>
      )}

      {queue.length > 1 && (
        <section className="w-full max-w-4xl bg-zinc-900 p-4 rounded-xl border border-zinc-800">
          <h2 className="font-semibold mb-3">Up next</h2>
          <ol className="space-y-2">
            {queue.slice(1).map((song, index) => {
              const queueIndex = index + 1;
              return (
                <li
                  key={`${song.video_id}-${queueIndex}`}
                  className="flex items-center gap-3 rounded-lg bg-zinc-800 p-2"
                >
                  <span className="w-6 text-center text-sm text-zinc-500">
                    {queueIndex}
                  </span>
                  <img
                    src={song.thumbnail_url}
                    alt=""
                    className="h-12 w-20 rounded object-cover"
                  />
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium">{song.title}</p>
                    <p className="truncate text-xs text-zinc-400">
                      {song.author_name}
                    </p>
                  </div>
                  <div className="flex shrink-0 gap-1">
                    <button
                      type="button"
                      onClick={() => moveInQueue(queueIndex, -1)}
                      disabled={queueIndex === 1}
                      aria-label={`Move ${song.title} up`}
                      className="rounded px-2 py-1 text-sm text-zinc-300 hover:bg-zinc-700 disabled:cursor-not-allowed disabled:opacity-30"
                    >
                      ↑
                    </button>
                    <button
                      type="button"
                      onClick={() => moveInQueue(queueIndex, 1)}
                      disabled={queueIndex === queue.length - 1}
                      aria-label={`Move ${song.title} down`}
                      className="rounded px-2 py-1 text-sm text-zinc-300 hover:bg-zinc-700 disabled:cursor-not-allowed disabled:opacity-30"
                    >
                      ↓
                    </button>
                    <button
                      type="button"
                      onClick={() => removeFromQueue(queueIndex)}
                      aria-label={`Remove ${song.title}`}
                      className="rounded px-2 py-1 text-sm text-red-300 hover:bg-red-950"
                    >
                      Remove
                    </button>
                  </div>
                </li>
              );
            })}
          </ol>
        </section>
      )}

      <Jukebox className="max-w-4xl" tiles={queue} spinning={!!nowPlaying} />
    </div>
  );
}

export default function LobbyPage() {
  return (
    <div className="relative w-full min-h-screen flex flex-col items-center justify-center bg-gradient-to-b from-black to-[#1a0b29] text-white px-6">
      <SignInGate returnTo="/lobby/">
        <Lobby />
      </SignInGate>
    </div>
  );
}
