"use client";

import { useEffect, useState } from "react";
import { listModes, startGame, type GameMode } from "../../lib/game";
import PromptEditor, { checkPrompts, newPrompt, type PromptDraft } from "./PromptEditor";
import SettingsForm, { draftFrom, parseDraft, type Draft } from "./SettingsForm";
import type { QueueSession, SessionSnapshot } from "../../lib/queue-state";

/** Host-only "Start a game" flow: pick a preset, customize its settings and prompts, start it. */
export default function GameSetup({ session, state, onClose }: {
  session: QueueSession; state: SessionSnapshot; onClose: () => void;
}) {
  const [modes, setModes] = useState<GameMode[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [selected, setSelected] = useState<GameMode | null>(null);
  const [draft, setDraft] = useState<Draft>({});
  const [prompts, setPrompts] = useState<PromptDraft[]>([]);
  const [error, setError] = useState<string | null>(null);
  const busy = state.gameBusy;

  useEffect(() => {
    const controller = new AbortController();
    listModes(controller.signal)
      .then(setModes)
      .catch(e => { if (!controller.signal.aborted) setLoadError(e instanceof Error ? e.message : "Could not load the games."); });
    return () => controller.abort();
  }, []);

  function choose(mode: GameMode) {
    setSelected(mode); setDraft(draftFrom(mode.settings)); setError(null);
    // Keep prompts already written (and their uploaded images) when switching modes.
    setPrompts(list => list.length ? list : mode.default_prompts.length
      ? mode.default_prompts.map(p => newPrompt(p.text ?? ""))
      : mode.rules.prompt === "none" ? [] : [newPrompt()]);
  }
  const parsed = selected ? parseDraft(selected.settings, draft) : null;
  const promptCheck = selected ? checkPrompts(selected.rules.prompt, prompts) : null;
  const ready = !!parsed?.valid && !promptCheck?.problem;
  const membershipId = state.membership?.membership_id ?? undefined;

  async function start(mode: GameMode) {
    if (!parsed?.valid || !promptCheck || promptCheck.problem) return;
    setError(null);
    const body = { mode_id: mode.id, settings: parsed.values, ...(mode.rules.prompt === "none" ? {} : { prompts: promptCheck.prompts }) };
    try {
      await session.gameMutate("Starting…", m => startGame(m, body));
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not start the game.");
      session.clearGameError();
    }
  }

  const presets = (modes ?? []).filter(m => m.preset);

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
              onClick={() => choose(mode)}
              aria-pressed={selected?.id === mode.id}
              className={`w-full h-full min-h-11 text-left rounded-lg border p-4 transition ${selected?.id === mode.id ? "border-[#FF0048] bg-[#5C172E]/60" : "border-zinc-700 bg-zinc-800 hover:border-[#FF0048]"}`}
            >
              <span className="block font-bold text-lg">{mode.name}</span>
              <span className="block text-sm text-zinc-300">{mode.description}</span>
            </button>
          </li>
        ))}
      </ul>
      {selected && (
        <div className="flex flex-col gap-3">
          <div className="flex items-baseline justify-between gap-3">
            <h3 className="text-lg font-bold">Customize {selected.name}</h3>
            <button type="button" onClick={() => setDraft(draftFrom(selected.settings))} className="min-h-11 px-2 text-sm text-zinc-400 underline hover:text-white">Reset</button>
          </div>
          {selected.rules.challenge === "guess_title" && (
            <div className="rounded-lg bg-amber-950/60 border border-amber-800 p-3 text-sm text-amber-200">
              <p>
                This device plays each clip in a normal YouTube player, and the player shows the title. Keep this screen out of
                the players&apos; sight (a laptop or phone by the speaker works) and put the board view on the TV instead.
                Phones only get the guess box.
              </p>
              <a href="/lobby/tv/" target="_blank" rel="noopener" className="inline-flex min-h-11 items-center font-semibold underline">Open the board view ↗</a>
            </div>
          )}
          <SettingsForm schema={selected.settings} draft={draft} onChange={setDraft} disabled={!!busy} />
          {selected.rules.prompt !== "none" && membershipId && (
            <PromptEditor membershipId={membershipId} rule={selected.rules.prompt} drafts={prompts} update={setPrompts} disabled={!!busy} />
          )}
          {parsed && !parsed.valid && <p className="text-sm text-red-400" role="status">Fix the highlighted settings to start.</p>}
          {parsed?.valid && promptCheck?.problem && <p className="text-sm text-amber-300" role="status">{promptCheck.problem}</p>}
        </div>
      )}
      {error && <p role="alert" className="text-red-400">The server said: {error}</p>}
      <button
        onClick={() => selected && void start(selected)}
        disabled={!selected || !!busy || !ready}
        className="min-h-11 px-6 rounded-lg bg-[#5C172E] hover:bg-[#FF0048] disabled:opacity-50 transition font-bold self-stretch sm:self-start"
      >
        {busy === "Starting…" ? "Starting…" : selected ? `Start ${selected.name}` : "Pick a game"}
      </button>
    </section>
  );
}
