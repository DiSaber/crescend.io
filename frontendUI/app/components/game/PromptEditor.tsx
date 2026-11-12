"use client";

import { GameImage } from "./bits";
import { deleteImage, IMAGE_MAX_BYTES, IMAGE_TYPES, uploadImage, type Prompt, type Rules } from "../../lib/game";

export type PromptDraft = { key: string; text: string; imageId?: string; uploading?: boolean; error?: string };

export const MAX_PROMPTS = 50;
export const MAX_PROMPT_TEXT = 200;
let next = 0;
export const newPrompt = (text = ""): PromptDraft => ({ key: `p${++next}`, text });

/** The prompts to send, or the first problem to fix (same limits as the server). */
export function checkPrompts(rule: Rules["prompt"], drafts: PromptDraft[]): { prompts: Prompt[]; problem: string | null } {
  if (rule === "none") return { prompts: [], problem: null };
  if (drafts.length === 0) return { prompts: [], problem: "Add at least one prompt." };
  if (drafts.length > MAX_PROMPTS) return { prompts: [], problem: `Use at most ${MAX_PROMPTS} prompts.` };
  const prompts: Prompt[] = [];
  for (const [i, d] of drafts.entries()) {
    const text = d.text.trim();
    if (d.uploading) return { prompts: [], problem: `Prompt ${i + 1} is still uploading.` };
    if (text.length > MAX_PROMPT_TEXT) return { prompts: [], problem: `Prompt ${i + 1} is over ${MAX_PROMPT_TEXT} characters.` };
    if (rule === "text") {
      if (!text) return { prompts: [], problem: `Prompt ${i + 1} needs text.` };
      prompts.push({ text });
    } else {
      if (!d.imageId) return { prompts: [], problem: `Prompt ${i + 1} needs an image.` };
      prompts.push(text ? { image_id: d.imageId, text } : { image_id: d.imageId });
    }
  }
  return { prompts, problem: null };
}

const kb = (bytes: number) => bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : `${Math.round(bytes / 1024)} KB`;

/**
 * Round prompts for prompt modes: text, plus a host-uploaded image in image modes.
 * Images upload as soon as they're chosen so the preview is the server's re-encoded
 * copy (metadata stripped), loaded with the bearer token like players will see it.
 */
export default function PromptEditor({ membershipId, rule, drafts, update, disabled }: {
  membershipId: string; rule: Rules["prompt"]; drafts: PromptDraft[];
  /** Functional updates: uploads finish after other edits and must patch the latest list. */
  update: (change: (drafts: PromptDraft[]) => PromptDraft[]) => void;
  disabled?: boolean;
}) {
  const patch = (key: string, change: Partial<PromptDraft>) =>
    update(list => list.map(d => d.key === key ? { ...d, ...change } : d));
  const move = (i: number, dir: -1 | 1) => update(list => {
    const copy = [...list]; [copy[i], copy[i + dir]] = [copy[i + dir], copy[i]]; return copy;
  });
  const remove = (d: PromptDraft) => {
    update(list => list.filter(x => x.key !== d.key));
    if (d.imageId) void deleteImage(membershipId, d.imageId).catch(() => {});
  };
  async function upload(d: PromptDraft, file: File) {
    if (!IMAGE_TYPES.includes(file.type)) { patch(d.key, { error: `“${file.name}” isn't a JPEG, PNG or WebP image.` }); return; }
    if (file.size > IMAGE_MAX_BYTES) { patch(d.key, { error: `“${file.name}” is ${kb(file.size)}; images can be up to 5 MB.` }); return; }
    const previous = d.imageId;
    patch(d.key, { uploading: true, error: undefined });
    try {
      const image = await uploadImage(membershipId, file);
      patch(d.key, { uploading: false, imageId: image.image_id });
      if (previous) void deleteImage(membershipId, previous).catch(() => {});
    } catch (e) {
      patch(d.key, { uploading: false, error: e instanceof Error ? e.message : "Upload failed. Try again." });
    }
  }

  return (
    <div className="flex flex-col gap-3">
      <div>
        <h3 className="text-lg font-bold">Prompts</h3>
        <p className="text-sm text-zinc-400">
          One per round, in this order.{rule === "image" ? " Each needs an image (JPEG, PNG or WebP, up to 5 MB); text is optional." : ""} Players see each one only when its round starts.
        </p>
      </div>
      <ol className="flex flex-col gap-3" aria-label="Prompts">
        {drafts.map((d, i) => (
          <li key={d.key} className="rounded-lg bg-zinc-800 p-3 flex flex-col gap-2">
            <div className="flex items-center justify-between gap-2">
              <span className="text-sm font-semibold">Prompt {i + 1}</span>
              <div className="flex gap-1">
                <button type="button" onClick={() => move(i, -1)} disabled={disabled || i === 0} aria-label={`Move prompt ${i + 1} up`} className="min-h-11 min-w-11 rounded-lg hover:bg-zinc-700 disabled:opacity-30">↑</button>
                <button type="button" onClick={() => move(i, 1)} disabled={disabled || i === drafts.length - 1} aria-label={`Move prompt ${i + 1} down`} className="min-h-11 min-w-11 rounded-lg hover:bg-zinc-700 disabled:opacity-30">↓</button>
                <button type="button" onClick={() => remove(d)} disabled={disabled} aria-label={`Delete prompt ${i + 1}`} className="min-h-11 px-3 rounded-lg text-red-300 hover:bg-red-950 disabled:opacity-50">Delete</button>
              </div>
            </div>
            {rule === "image" && (
              <div className="flex flex-col gap-2">
                {d.imageId && <GameImage membershipId={membershipId} imageId={d.imageId} alt={`Prompt ${i + 1} image`} className="w-full max-h-64 min-h-32 rounded-lg object-contain bg-black" />}
                <label className={`inline-flex min-h-11 items-center justify-center self-start rounded-lg px-4 font-semibold ${disabled || d.uploading ? "bg-zinc-700 opacity-60" : "bg-zinc-700 hover:bg-zinc-600 cursor-pointer"}`}>
                  {d.uploading ? "Uploading…" : d.imageId ? "Replace image" : "Upload image"}
                  <input
                    type="file"
                    accept={IMAGE_TYPES.join(",")}
                    disabled={disabled || d.uploading}
                    aria-label={`Image for prompt ${i + 1}`}
                    className="sr-only"
                    onChange={e => { const f = e.target.files?.[0]; e.target.value = ""; if (f) void upload(d, f); }}
                  />
                </label>
                {d.error && <p role="alert" className="text-sm text-red-400">{d.error}</p>}
              </div>
            )}
            <input
              value={d.text}
              onChange={e => patch(d.key, { text: e.target.value })}
              maxLength={MAX_PROMPT_TEXT}
              disabled={disabled}
              aria-label={`Prompt ${i + 1} text`}
              placeholder={rule === "image" ? "Caption (optional), e.g. Which song is most this?" : "e.g. A song for a rainy day"}
              className="min-h-11 w-full min-w-0 rounded-lg bg-zinc-900 border border-zinc-700 px-3 placeholder-zinc-500"
            />
          </li>
        ))}
      </ol>
      <button
        type="button"
        onClick={() => update(list => [...list, newPrompt()])}
        disabled={disabled || drafts.length >= MAX_PROMPTS}
        className="min-h-11 self-start rounded-lg border border-dashed border-zinc-600 px-4 hover:border-[#FF0048] disabled:opacity-50"
      >
        + Add prompt
      </button>
    </div>
  );
}
