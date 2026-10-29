"use client";

import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { useRouter } from "next/navigation";
import Jukebox from "../components/jukebox.jsx";
import SignInGate from "../components/SignInGate";
import { currentUserId } from "../lib/auth";
import {
  getCurrent,
  leaveLobby,
  streamLobbyEvents,
  type Lobby as LobbyState,
  type VideoMetadata,
} from "../lib/lobby";
import {
  advance,
  getQueue,
  moveEntry,
  newer,
  newRequestId,
  QueueError,
  setVotingPolicy,
  submitEntry,
  veto,
  vote,
  type AdvanceReason,
  type QueueEntry,
  type QueueView,
} from "../lib/queue";

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

/**
 * Host-only player. Remount it per queue entry (key = entry ID) so a repeated video
 * still gets its own ended/error callback, and each callback names the occurrence it saw.
 */
function YouTubePlayer({
  video,
  onEnded,
  onVideoError,
  onLoadError,
}: {
  video: VideoMetadata;
  onEnded: () => void;
  onVideoError: () => void;
  onLoadError: (message: string) => void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const reported = useRef(false);
  // Keep the latest callbacks without recreating the player on every render.
  const callbacks = useRef({ onEnded, onVideoError, onLoadError });
  useEffect(() => {
    callbacks.current = { onEnded, onVideoError, onLoadError };
  });

  useEffect(() => {
    let player: YouTubePlayerInstance | undefined;
    let cancelled = false;
    reported.current = false;
    const once = (fn: () => void) => {
      if (cancelled || reported.current) return;
      reported.current = true;
      fn();
    };

    loadYouTubeApi()
      .then((youtube) => {
        if (cancelled || !container.current) return;
        const endedState = youtube.PlayerState?.ENDED ?? 0;
        player = new youtube.Player(container.current, {
          videoId: video.video_id,
          playerVars: {
            autoplay: 1,
            enablejsapi: 1,
            origin: window.location.origin,
          },
          events: {
            onStateChange: (event) => {
              if (event.data === endedState) once(() => callbacks.current.onEnded());
            },
            onError: () => once(() => callbacks.current.onVideoError()),
          },
        });
      })
      .catch(() => {
        if (!cancelled) callbacks.current.onLoadError("Could not load the YouTube player.");
      });

    return () => {
      cancelled = true;
      player?.destroy();
    };
  }, [video.video_id]);

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
  const [queue, setQueue] = useState<QueueView | null>(null);
  const [userId, setUserId] = useState<string>();
  const [ended, setEnded] = useState<string>();
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [live, setLive] = useState(false);
  const [busy, setBusy] = useState<string>();

  const [link, setLink] = useState("");
  const [adding, setAdding] = useState(false);
  // One request ID per submission attempt: reused when retrying the same link after an
  // uncertain failure, replaced once the link changes or the submission is settled.
  const pendingSubmit = useRef<{ url: string; requestId: string } | undefined>(undefined);

  // The membership generation and queue revision we last applied. Responses for any
  // other generation (left/rejoined) or with a lower revision are stale and ignored.
  const epoch = useRef<string | null | undefined>(undefined);
  const queueRevision = useRef<string | undefined>(undefined);

  useEffect(() => {
    currentUserId().then(setUserId);
  }, []);

  const applyQueue = useCallback((view: QueueView) => {
    if (view.membership_id !== epoch.current) return;
    if (!newer(view.revision, queueRevision.current)) return;
    queueRevision.current = view.revision;
    setQueue(view);
  }, []);

  const applyMembership = useCallback((id: string | null, nextLobby: LobbyState | null) => {
    if (id !== epoch.current) {
      epoch.current = id;
      queueRevision.current = undefined;
      setQueue(null);
    }
    setMembershipId(id);
    setLobby(nextLobby);
  }, []);

  // Serialized refetch of membership + queue: a hint while a read is in flight triggers
  // one more read after it. A queue 403 means our generation is gone, so the next pass
  // rereads membership to recover the current one.
  const inFlight = useRef(false);
  const dirty = useRef(false);
  const sync = useCallback(async () => {
    dirty.current = true;
    if (inFlight.current) return;
    inFlight.current = true;
    let recoveries = 0;
    try {
      while (dirty.current) {
        dirty.current = false;
        const current = await getCurrent();
        applyMembership(current.membership_id, current.lobby);
        if (!current.membership_id) continue;
        try {
          applyQueue(await getQueue(current.membership_id));
        } catch (e) {
          if (e instanceof QueueError && e.status === 403 && recoveries++ < 2) {
            dirty.current = true;
          } else {
            throw e;
          }
        }
      }
      setError(undefined);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      inFlight.current = false;
    }
  }, [applyMembership, applyQueue]);

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
            // Both hints invalidate membership and queue; sync() rereads both.
            if (e.event === "sync_required" || e.event === "lobby_changed") sync();
            if (e.event === "membership_ended") {
              stop = true;
              setEnded(ENDED_MESSAGES[e.data.reason]);
              if (epoch.current === membershipId) applyMembership(null, null);
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
  }, [membershipId, sync, applyMembership]);

  /**
   * Run one queue mutation against the current generation. Apply its snapshot when it
   * is still for our generation; on conflicts, lost membership or uncertain outcomes,
   * refetch instead of guessing.
   */
  const mutate = useCallback(
    async (label: string, op: (membership: string) => Promise<{ queue: QueueView }>) => {
      const membership = epoch.current;
      if (!membership) return undefined;
      setBusy(label);
      try {
        const result = await op(membership);
        applyQueue(result.queue);
        setError(undefined);
        return result;
      } catch (e) {
        if (e instanceof QueueError) {
          if (e.status === 409 || e.status === 403 || e.status === 404) sync();
          if (e.code !== "current_entry_changed" && e.code !== "stale_queue_revision") setError(e.message);
        } else {
          setError((e as Error).message);
          sync();
        }
        throw e;
      } finally {
        setBusy(undefined);
      }
    },
    [applyQueue, sync],
  );

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
    const url = link.trim();
    if (!url) return;
    if (pendingSubmit.current?.url !== url) pendingSubmit.current = { url, requestId: newRequestId() };
    const { requestId } = pendingSubmit.current;
    setAdding(true);
    try {
      await mutate("add", (m) => submitEntry(m, url, requestId));
      pendingSubmit.current = undefined;
      setLink("");
    } catch (err) {
      // Definite rejections settle the attempt; network/5xx keep the ID for a safe retry.
      if (err instanceof QueueError && err.status < 500) pendingSubmit.current = undefined;
    } finally {
      setAdding(false);
    }
  }

  // Host progression names the occurrence it saw end; a 409 means someone else already
  // moved the queue, so mutate() refetches and the player follows the new current.
  const advanceFrom = useCallback(
    (entry: QueueEntry, reason: AdvanceReason) => {
      mutate(`advance-${entry.id}`, (m) => advance(m, entry.id, reason)).catch(() => {});
    },
    [mutate],
  );

  const reportLoadError = useCallback((message: string) => setError(message), []);

  const current = queue?.current ?? null;
  const upcoming = queue?.upcoming ?? [];
  const isHost = !!lobby && !!userId && lobby.owner_user_id === userId;
  const threshold = queue?.voting.effective_threshold ?? null;

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

  const contributor = (entry: QueueEntry) =>
    entry.submitted_by_user_id === userId
      ? "you"
      : entry.submitted_by_user_id === lobby.owner_user_id
        ? "the host"
        : `Guest #${entry.submitted_by_user_id}`;

  const voteLabel = (entry: QueueEntry) =>
    threshold === null ? `${entry.active_vote_count} skip votes` : `${entry.active_vote_count}/${threshold} to skip`;

  const entryControls = (entry: QueueEntry, index?: number) => {
    return (
      <div className="flex shrink-0 items-center gap-1">
        <span className="px-2 text-xs text-zinc-400" title="Skip votes from guests">
          {voteLabel(entry)}
        </span>
        {!isHost && (
          <button
            type="button"
            onClick={() => mutate(`vote-${entry.id}`, (m) => vote(m, entry.id)).catch(() => {})}
            disabled={entry.has_voted || !!busy}
            className="rounded px-2 py-1 text-sm text-zinc-200 bg-zinc-700 hover:bg-zinc-600 disabled:opacity-50"
          >
            {entry.has_voted ? "Voted" : "Vote skip"}
          </button>
        )}
        {isHost && index !== undefined && (
          <>
            <button
              type="button"
              onClick={() => mutate(`move-${entry.id}`, (m) => moveEntry(m, entry.id, queue!.revision, index - 1)).catch(() => {})}
              disabled={index === 0 || !!busy}
              aria-label={`Move ${entry.video.title} up`}
              className="rounded px-2 py-1 text-sm text-zinc-300 hover:bg-zinc-700 disabled:cursor-not-allowed disabled:opacity-30"
            >
              ↑
            </button>
            <button
              type="button"
              onClick={() => mutate(`move-${entry.id}`, (m) => moveEntry(m, entry.id, queue!.revision, index + 1)).catch(() => {})}
              disabled={index === upcoming.length - 1 || !!busy}
              aria-label={`Move ${entry.video.title} down`}
              className="rounded px-2 py-1 text-sm text-zinc-300 hover:bg-zinc-700 disabled:cursor-not-allowed disabled:opacity-30"
            >
              ↓
            </button>
          </>
        )}
        {isHost && (
          <button
            type="button"
            onClick={() => mutate(`veto-${entry.id}`, (m) => veto(m, entry.id)).catch(() => {})}
            disabled={!!busy}
            aria-label={`Veto ${entry.video.title}`}
            className="rounded px-2 py-1 text-sm text-red-300 hover:bg-red-950 disabled:opacity-50"
          >
            Veto
          </button>
        )}
      </div>
    );
  };

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
                {m.user_id === userId && " (you)"}
              </li>
            ))}
          </ul>
          {isHost && queue && (
            <VotingPolicy
              voting={queue.voting}
              disabled={!!busy}
              onChange={(policy) => mutate("policy", (m) => setVotingPolicy(m, policy)).catch(() => {})}
            />
          )}
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
      {notice && (
        <p className="text-amber-300 text-sm">
          {notice}{" "}
          <button type="button" onClick={() => setNotice(undefined)} className="underline text-zinc-400">dismiss</button>
        </p>
      )}

      {current ? (
        <div className="w-full max-w-4xl bg-zinc-900 p-4 rounded-xl border border-zinc-800">
          <div className="flex justify-between items-center gap-4 mb-3">
            <div className="min-w-0">
              <p className="font-semibold truncate">Now playing: {current.video.title}</p>
              <p className="text-xs text-zinc-400 truncate">
                {current.video.author_name} · added by {contributor(current)}
              </p>
            </div>
            <div className="flex items-center gap-1">
              {isHost && (
                <button
                  onClick={() => advanceFrom(current, "skip")}
                  disabled={!!busy}
                  className="text-sm underline text-zinc-400 hover:text-white shrink-0 mr-2 disabled:opacity-50"
                >
                  Skip
                </button>
              )}
              {entryControls(current)}
            </div>
          </div>
          {isHost ? (
            <YouTubePlayer
              key={current.id}
              video={current.video}
              onEnded={() => advanceFrom(current, "ended")}
              onVideoError={() => {
                setNotice(`Skipped "${current.video.title}": YouTube could not play it here.`);
                advanceFrom(current, "playback_error_skip");
              }}
              onLoadError={reportLoadError}
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
          <p className="mt-2 text-sm text-zinc-500">
            Add a YouTube link above to start playback
            {isHost ? " on this device." : " on the host's device."}
          </p>
        </div>
      )}

      {upcoming.length > 0 && (
        <section className="w-full max-w-4xl bg-zinc-900 p-4 rounded-xl border border-zinc-800">
          <h2 className="font-semibold mb-3">Up next</h2>
          <ol className="space-y-2">
            {upcoming.map((entry, index) => (
              <li key={entry.id} className="flex items-center gap-3 rounded-lg bg-zinc-800 p-2">
                <span className="w-6 text-center text-sm text-zinc-500">{index + 1}</span>
                <img src={entry.video.thumbnail_url} alt="" className="h-12 w-20 rounded object-cover" />
                <div className="min-w-0 flex-1">
                  <p className="truncate text-sm font-medium">{entry.video.title}</p>
                  <p className="truncate text-xs text-zinc-400">
                    {entry.video.author_name} · added by {contributor(entry)}
                  </p>
                </div>
                {entryControls(entry, index)}
              </li>
            ))}
          </ol>
        </section>
      )}

      <Jukebox
        className="max-w-4xl"
        tiles={[...(current ? [current] : []), ...upcoming].map((entry) => entry.video)}
        spinning={!!current}
      />
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
  const [count, setCount] = useState(String(voting.fixed_count ?? 2));
  return (
    <div className="mt-4 flex flex-wrap items-center gap-2 text-sm text-zinc-300">
      <span className="text-zinc-400">Vote skip needs</span>
      <select
        value={voting.mode}
        disabled={disabled}
        onChange={(e) =>
          onChange(e.target.value === "fixed" ? { mode: "fixed", count: Math.max(1, Number(count) || 1) } : { mode: "majority" })
        }
        className="rounded bg-zinc-800 border border-zinc-700 px-2 py-1"
      >
        <option value="majority">a majority of guests</option>
        <option value="fixed">a fixed number of votes</option>
      </select>
      {voting.mode === "fixed" && (
        <input
          type="number"
          min={1}
          value={count}
          disabled={disabled}
          onChange={(e) => setCount(e.target.value)}
          onBlur={() => {
            const n = Math.floor(Number(count));
            if (n >= 1 && n !== voting.fixed_count) onChange({ mode: "fixed", count: n });
          }}
          className="w-16 rounded bg-zinc-800 border border-zinc-700 px-2 py-1"
        />
      )}
      <span className="text-zinc-500">
        ({voting.active_guest_count} guest{voting.active_guest_count === 1 ? "" : "s"}
        {voting.effective_threshold !== null && `, ${voting.effective_threshold} needed`})
      </span>
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
