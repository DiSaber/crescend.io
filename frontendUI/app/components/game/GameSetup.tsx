"use client";

import { useEffect, useState } from "react";
import { deleteMode, limitModeName, listModes, modeNameError, modeNameLength, playerWarning, saveMode, serverSettingErrors, startGame, type GameMode } from "../../lib/game";
import PromptEditor, { checkPrompts, newPrompt, type PromptDraft } from "./PromptEditor";
import SettingsForm, { draftFrom, parseDraft, type Draft } from "./SettingsForm";
import { normalizeNickname } from "../../lib/lobby";
import type { QueueSession, SessionSnapshot } from "../../lib/queue-state";

/**
 * Host-only "Start a game" flow: pick a preset or one of My modes, customize its
 * settings and prompts, optionally save it as a private mode, start it.
 */
export default function GameSetup({ session, state, onClose }: {
  session: QueueSession; state: SessionSnapshot; onClose: () => void;
}) {
  const [modes, setModes] = useState<GameMode[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [selected, setSelected] = useState<GameMode | null>(null);
  const [draft, setDraft] = useState<Draft>({});
  const [prompts, setPrompts] = useState<PromptDraft[]>([]);
  const [error, setError] = useState<string | null>(null);
  /** The server's per-setting rejection; cleared as soon as the host edits a setting. */
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  // Any edit (or Reset) means the server's last word no longer applies.
  const editDraft = (next: Draft) => { setDraft(next); setFieldErrors({}); setError(null); setSaving(s => (s.error ? {} : s)); };
  const [saveName, setSaveName] = useState("");
  const [nameCut, setNameCut] = useState(false);
  const editName = (text: string) => {
    const limited = limitModeName(text);
    setSaveName(limited.value); setNameCut(limited.truncated);
    setSaving(s => (s.error ? {} : s));
    // A new name is a new attempt: the last start rejection no longer applies.
    setError(null);
  };
  const [saving, setSaving] = useState<{ busy?: boolean; message?: string; error?: string }>({});
  const busy = state.gameBusy;

  useEffect(() => {
    const controller = new AbortController();
    listModes(controller.signal)
      .then(setModes)
      .catch(e => { if (!controller.signal.aborted) setLoadError(e instanceof Error ? e.message : "Could not load the games."); });
    return () => controller.abort();
  }, []);

  function choose(mode: GameMode) {
    setSelected(mode); setDraft(draftFrom(mode.settings)); setError(null); setSaving({}); setFieldErrors({});
    // A saved mode's default prompts win; otherwise keep prompts already written
    // (and their uploaded images) when switching modes.
    setPrompts(list => mode.default_prompts.length
      ? mode.default_prompts.map(p => newPrompt(p.text ?? ""))
      : list.length || mode.rules.prompt === "none" ? list : [newPrompt()]);
  }
  const parsed = selected ? parseDraft(selected.settings, draft) : null;
  const promptCheck = selected ? checkPrompts(selected.rules.prompt, prompts) : null;
  const players = state.membership?.lobby?.members.length ?? 0;
  const warning = selected ? playerWarning(selected, players) : null;
  const ready = !!parsed?.valid && !promptCheck?.problem && !warning?.blocks;
  const membershipId = state.membership?.membership_id ?? undefined;
  const nameProblem = saveName ? modeNameError(saveName) : null;
  // Names the fields to fix next to Start: local checks, then the server's rejection.
  const badFields = selected
    ? selected.settings.filter(s => parsed?.errors[s.key] || fieldErrors[s.key]).map(s => s.label)
    : [];

  async function start(mode: GameMode) {
    if (!parsed?.valid || !promptCheck || promptCheck.problem) return;
    setError(null); setSaving({});
    const body = { mode_id: mode.id, settings: parsed.values, ...(mode.rules.prompt === "none" ? {} : { prompts: promptCheck.prompts }) };
    try {
      await session.gameMutate("Starting…", m => startGame(m, body));
      onClose();
    } catch (e) {
      const fields = serverSettingErrors(e);
      setFieldErrors(fields);
      setError(fieldMessage(mode, fields) ?? (e instanceof Error ? e.message : "Could not start the game."));
      session.clearGameError();
    }
  }

  async function save(mode: GameMode) {
    const name = normalizeNickname(saveName);
    if (!parsed?.valid || !name || modeNameError(name)) return;
    // Only text prompts can be kept: uploaded images belong to this lobby.
    const keep = mode.rules.prompt === "text" && promptCheck && !promptCheck.problem ? promptCheck.prompts : undefined;
    setSaving({ busy: true });
    try {
      const saved = await saveMode({ name, rules: mode.rules, settings: parsed.values, ...(keep ? { prompts: keep } : {}) });
      setModes(list => [...(list ?? []), saved]);
      // The server makes a repeated name unique (" 2"), so say what it was saved as.
      const preset = (modes ?? []).some(m => m.preset && m.name.toLowerCase() === name.toLowerCase());
      setSaving({
        message: saved.name === name ? `Saved “${saved.name}” to My modes.`
          : preset ? `That name is taken by a built-in mode, so this one is saved as “${saved.name}”.`
          : `You already have “${name}”, so this one is saved as “${saved.name}”.`,
      });
      setSaveName(""); setNameCut(false); setFieldErrors({}); setError(null);
    } catch (e) {
      const fields = serverSettingErrors(e);
      setFieldErrors(fields);
      setSaving({ error: fieldMessage(mode, fields) ?? (e instanceof Error ? e.message : "Could not save the mode.") });
    }
  }
  async function remove(mode: GameMode) {
    if (!window.confirm(`Delete “${mode.name}” from My modes?`)) return;
    try {
      await deleteMode(mode.id);
      setModes(list => (list ?? []).filter(m => m.id !== mode.id));
      if (selected?.id === mode.id) setSelected(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not delete the mode.");
    }
  }

  const presets = (modes ?? []).filter(m => m.preset);
  const mine = (modes ?? []).filter(m => !m.preset);

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
      {mine.length > 0 && (
        <div className="flex flex-col gap-2">
          <h3 className="text-lg font-bold">My modes</h3>
          <ul className="flex flex-col gap-2" aria-label="My modes">
            {mine.map(mode => (
              <li key={mode.id} className={`flex flex-wrap items-center gap-2 rounded-lg border p-3 ${selected?.id === mode.id ? "border-[#FF0048] bg-[#5C172E]/60" : "border-zinc-700 bg-zinc-800"}`}>
                <div className="min-w-0 flex-1 basis-40">
                  <p className="font-bold truncate" title={mode.name}>{mode.name}</p>
                  <p className="text-xs text-zinc-400">{summary(mode)}</p>
                </div>
                <button onClick={() => choose(mode)} aria-label={`Use ${mode.name}`} className="min-h-11 px-4 rounded-lg bg-zinc-700 hover:bg-zinc-600 font-semibold">Use</button>
                <button onClick={() => void remove(mode)} aria-label={`Delete ${mode.name}`} className="min-h-11 px-3 rounded-lg text-red-300 hover:bg-red-950">Delete</button>
              </li>
            ))}
          </ul>
        </div>
      )}
      {selected && (
        <div className="flex flex-col gap-3">
          <div className="flex items-baseline justify-between gap-3">
            <h3 className="text-lg font-bold">Customize {selected.name}</h3>
            <button type="button" onClick={() => { editDraft(draftFrom(selected.settings)); setSaving({}); }} className="min-h-11 px-2 text-sm text-zinc-400 underline hover:text-white">Reset</button>
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
          {warning && (
            <p role="status" className={`rounded-lg border p-3 text-sm ${warning.blocks ? "bg-red-950/60 border-red-800 text-red-200" : "bg-amber-950/60 border-amber-800 text-amber-200"}`}>{warning.text}</p>
          )}
          <SettingsForm schema={selected.settings} draft={draft} onChange={editDraft} disabled={!!busy} serverErrors={fieldErrors} />
          {selected.rules.prompt !== "none" && membershipId && (
            <PromptEditor membershipId={membershipId} rule={selected.rules.prompt} drafts={prompts} update={setPrompts} disabled={!!busy} />
          )}
          <form onSubmit={e => { e.preventDefault(); void save(selected); }} className="flex flex-col gap-2 rounded-lg border border-zinc-800 p-3">
            <label htmlFor="save-mode-name" className="text-sm font-semibold">Save as my mode</label>
            <div className="flex flex-col sm:flex-row gap-2">
              {/* No maxLength: it counts UTF-16 units, so an emoji would use two of the 40. */}
              <input id="save-mode-name" value={saveName} onChange={e => editName(e.target.value)} aria-invalid={!!nameProblem} aria-describedby="save-mode-hint" placeholder={`e.g. ${selected.name} night`} className="min-h-11 min-w-0 flex-1 rounded-lg bg-zinc-800 border border-zinc-700 px-3 placeholder-zinc-500" />
              <button type="submit" disabled={!saveName.trim() || !!nameProblem || !parsed?.valid || saving.busy} className="min-h-11 px-4 rounded-lg bg-zinc-700 hover:bg-zinc-600 font-semibold disabled:opacity-50">{saving.busy ? "Saving…" : "Save"}</button>
            </div>
            <p id="save-mode-hint" className={`text-xs ${nameProblem || nameCut ? "text-amber-300" : "text-zinc-400"}`} role={nameProblem || nameCut ? "status" : undefined}>
              {nameProblem ?? (nameCut ? "Names are up to 40 characters, so the rest was cut." : `${modeNameLength(saveName)}/40 characters`)}
            </p>
            <p className="text-xs text-zinc-400">
              Saves these settings privately for your account.{selected.rules.prompt === "image" ? " Images aren't saved; add them each time." : selected.rules.prompt === "text" ? " Text prompts are saved too." : ""}
            </p>
            {saving.message && <p role="status" className="text-sm text-green-400">{saving.message}</p>}
            {saving.error && <p role="alert" className="text-sm text-red-400">{saving.error}</p>}
          </form>
          {badFields.length > 0 && <p className="text-sm text-red-400" role="status">Fix {listLabels(badFields)} to start.</p>}
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

/** “Rounds” and “Voting time”: labels as one phrase. */
function listLabels(labels: string[]) {
  const quoted = labels.map(l => `“${l}”`);
  return quoted.length < 2 ? quoted.join("") : `${quoted.slice(0, -1).join(", ")} and ${quoted[quoted.length - 1]}`;
}

/** "Rounds: Must be between 1 and 10." for a server rejection that names one setting. */
function fieldMessage(mode: GameMode, fields: Record<string, string>) {
  const [key, text] = Object.entries(fields)[0] ?? [];
  if (!key) return null;
  return `${mode.settings.find(s => s.key === key)?.label ?? key}: ${text}`;
}

const kinds: Record<string, string> = { all: "vote", pairs: "duel", none: "guess the title" };
/** One line for a saved mode card: its kind and the settings that matter most. */
function summary(mode: GameMode) {
  const value = (key: string) => mode.settings.find(s => s.key === key)?.default;
  const rounds = value("rounds");
  const kind = mode.rules.prompt === "image" ? "picture prompts" : mode.rules.prompt === "text" ? "text prompts" : kinds[mode.rules.ballot];
  const all = mode.rules.prompt !== "none" ? "a round per prompt" : mode.rules.collect === "propose" ? "every song" : "until stopped";
  return `${kind} · ${rounds ? `${rounds} rounds` : all}${mode.default_prompts.length ? ` · ${mode.default_prompts.length} prompts` : ""}`;
}
