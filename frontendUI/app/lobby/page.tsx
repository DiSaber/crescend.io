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

      {nowPlaying && (
        <div className="w-full max-w-4xl bg-zinc-900 p-4 rounded-xl border border-zinc-800">
          <div className="flex justify-between items-center mb-3">
            <p className="font-semibold truncate">Now playing: {nowPlaying.title}</p>
            <button onClick={() => setQueue((q) => q.slice(1))} className="text-sm underline text-zinc-400 hover:text-white shrink-0 ml-4">
              Skip
            </button>
          </div>
          <iframe
            key={nowPlaying.video_id}
            src={nowPlaying.embed_url}
            allow="autoplay; encrypted-media"
            className="w-full aspect-video rounded-lg"
          />
          <p className="mt-2 text-xs text-zinc-500">The queue is only on this device for now; a shared queue needs backend support.</p>
        </div>
      )}

      <Jukebox scale={0.9} tiles={queue} spinning={!!nowPlaying} />
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
