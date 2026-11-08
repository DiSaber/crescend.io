"use client";

import { nudgeValue, settingError, type SettingSchema, type SettingValues } from "../../lib/game";

/** Raw text for integer fields so hosts can type freely; parsed on every change. */
export type Draft = Record<string, string | boolean>;

export function draftFrom(schema: SettingSchema[], values?: SettingValues): Draft {
  return Object.fromEntries(schema.map(s => {
    const v = values?.[s.key] ?? s.default;
    return [s.key, s.type === "boolean" ? Boolean(v) : String(v)];
  }));
}

/** The values to send, plus a client-side error per invalid key (same rules as the server). */
export function parseDraft(schema: SettingSchema[], draft: Draft) {
  const values: SettingValues = {};
  const errors: Record<string, string> = {};
  for (const spec of schema) {
    const raw = draft[spec.key];
    const value = spec.type === "boolean" ? raw : /^\s*-?\d+\s*$/.test(String(raw)) ? Number(raw) : NaN;
    const error = settingError(spec, value);
    if (error) errors[spec.key] = error; else values[spec.key] = value as number | boolean;
  }
  return { values, errors, valid: Object.keys(errors).length === 0 };
}

const unitShort = (unit: string | null) => unit === "seconds" ? "s" : unit ?? "";

/** The customize form, generated from a mode's served settings schema. */
export default function SettingsForm({ schema, draft, onChange, disabled, serverErrors }: {
  schema: SettingSchema[]; draft: Draft; onChange: (draft: Draft) => void; disabled?: boolean;
  /** Per-key messages from the server's last rejection; local checks win when both apply. */
  serverErrors?: Record<string, string>;
}) {
  const { errors: local } = parseDraft(schema, draft);
  const errors = { ...serverErrors, ...local };
  const set = (key: string, value: string | boolean) => onChange({ ...draft, [key]: value });
  return (
    <fieldset className="flex flex-col gap-3" disabled={disabled}>
      <legend className="sr-only">Settings</legend>
      {schema.map(spec => {
        const id = `setting-${spec.key}`;
        if (spec.type === "boolean") {
          return (
            <div key={spec.key} className={`rounded-lg bg-zinc-800 ${errors[spec.key] ? "ring-1 ring-red-500" : ""}`}>
              <label htmlFor={id} className="flex min-h-11 items-center gap-3 px-3 py-2 cursor-pointer">
                <input id={id} type="checkbox" checked={draft[spec.key] === true} onChange={e => set(spec.key, e.target.checked)}
                  aria-invalid={!!errors[spec.key]} aria-describedby={errors[spec.key] ? `${id}-hint` : undefined} className="size-11 shrink-0 sm:size-6 accent-[#FF0048] cursor-pointer" />
                <span className="text-sm">{spec.label}</span>
              </label>
              {errors[spec.key] && <p id={`${id}-hint`} className="px-3 pb-2 text-xs text-red-400">{errors[spec.key]}</p>}
            </div>
          );
        }
        const raw = String(draft[spec.key] ?? "");
        const n = /^\s*-?\d+\s*$/.test(raw) ? Number(raw) : NaN;
        const step = spec.step ?? 1, min = spec.min ?? 0, max = spec.max ?? Number.MAX_SAFE_INTEGER;
        // Disabled only at the edge itself: out of range, both buttons bring it back inside.
        const top = min + Math.floor((max - min) / step) * step;
        const atBottom = n === min, atTop = n >= top && n <= max;
        // One step in the pressed direction, landing on the step grid.
        const nudge = (dir: 1 | -1) => set(spec.key, String(nudgeValue(spec, raw, dir)));
        const error = errors[spec.key];
        return (
          <div key={spec.key} className="rounded-lg bg-zinc-800 px-3 py-2">
            <label htmlFor={id} className="block text-sm">{spec.label}</label>
            <div className="mt-1 flex items-center gap-2">
              <button type="button" onClick={() => nudge(-1)} aria-label={`Less: ${spec.label}`} disabled={atBottom} className="min-h-11 min-w-11 rounded-lg bg-zinc-700 text-lg disabled:opacity-40">−</button>
              <input
                id={id}
                inputMode="numeric"
                value={raw}
                onChange={e => set(spec.key, e.target.value)}
                aria-invalid={!!error}
                aria-describedby={`${id}-hint`}
                className={`min-h-11 w-20 min-w-0 rounded-lg bg-zinc-900 border px-2 text-center font-mono ${error ? "border-red-500" : "border-zinc-700"}`}
              />
              <button type="button" onClick={() => nudge(1)} aria-label={`More: ${spec.label}`} disabled={atTop} className="min-h-11 min-w-11 rounded-lg bg-zinc-700 text-lg disabled:opacity-40">+</button>
              <span className="text-sm text-zinc-400">{unitShort(spec.unit)}</span>
            </div>
            <p id={`${id}-hint`} className={`mt-1 text-xs ${error ? "text-red-400" : "text-zinc-400"}`}>
              {error ?? `${min}–${max}${unitShort(spec.unit) === "s" ? " s" : ""}${step > 1 ? `, steps of ${step}` : ""}`}
            </p>
          </div>
        );
      })}
    </fieldset>
  );
}
