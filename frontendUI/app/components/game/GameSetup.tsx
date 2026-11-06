"use client";

import { useEffect, useState } from "react";
import { listModes, startGame, type GameMode } from "../../lib/game";
import type { QueueSession, SessionSnapshot } from "../../lib/queue-state";

/** Host-only "Start a game" flow: pick a preset, then start it. */
export default function GameSetup({ session, state, onClose }: {
  session: QueueSession; state: SessionSnapshot; onClose: () => void;
}) {
  const [modes, setModes] = useState<GameMode[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [selected, setSelected] = useState<GameMode | null>(null);
  const [error, setError] = useState<string | null>(null);
  const busy = state.gameBusy;

  useEffect(() => {
    const controller = new AbortController();
    listModes(controller.signal)
      .then(setModes)
      .catch(e => { if (!controller.signal.aborted) setLoadError(e instanceof Error ? e.message : "Could not load the games."); });
    return () => controller.abort();
  }, []);

  async function start(mode: GameMode) {
    setError(null);
    try {
      await session.gameMutate("Starting…", m => startGame(m, { mode_id: mode.id }));
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not start the game.");
      session.clearGameError();
    }
  }

  const presets = (modes ?? []).filter(m => m.preset && m.rules.prompt === "none" && m.rules.collect === "pick");

  return (
    <section aria-label="Start a game" className="w-full max-w-4xl bg-zinc-900 p-4 sm:p-6 rounded-xl border border-[#5C172E] flex flex-col gap-4">
      <header className="flex items-center justify-between gap-3">
        <h2 className="text-2xl font-extrabold">Start a game</h2>
        <button onClick={onClose} className="min-h-11 px-3 text-sm text-zinc-400 underline hover:text-white">Cancel</button>
      </header>
      {!modes && !loadError && <p role="status" className="text-zinc-400">Loading games…</p>}
      {loadError && <p role="alert" className="text-red-400">{loadError}</p>}
      <ul className="grid gap-3 sm:grid-cols-2" aria-label="Presets">
        {presets.map(mode => (
          <li key={mode.id}>
            <button
              onClick={() => setSelected(mode)}
              aria-pressed={selected?.id === mode.id}
              className={`w-full h-full min-h-11 text-left rounded-lg border p-4 transition ${selected?.id === mode.id ? "border-[#FF0048] bg-[#5C172E]/60" : "border-zinc-700 bg-zinc-800 hover:border-[#FF0048]"}`}
            >
              <span className="block font-bold text-lg">{mode.name}</span>
              <span className="block text-sm text-zinc-300">{mode.description}</span>
            </button>
          </li>
        ))}
      </ul>
      {error && <p role="alert" className="text-red-400">{error}</p>}
      <button
        onClick={() => selected && void start(selected)}
        disabled={!selected || !!busy}
        className="min-h-11 px-6 rounded-lg bg-[#5C172E] hover:bg-[#FF0048] disabled:opacity-50 transition font-bold self-stretch sm:self-start"
      >
        {busy === "Starting…" ? "Starting…" : selected ? `Start ${selected.name}` : "Pick a game"}
      </button>
    </section>
  );
}
