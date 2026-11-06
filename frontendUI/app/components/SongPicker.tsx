"use client";

import type { FormEvent } from "react";
import SongSearch from "./SongSearch";
import { youTubeVideoUrl } from "../lib/youtube";

/**
 * The lobby's "song name or YouTube link" box: a pasted link submits directly, other
 * text searches the catalog (#51). Shared by the jukebox queue and game picks.
 */
export default function SongPicker({
  value, onChange, onSubmit, addingUrl, disabled = false, inputDisabled = false, label = "Song name or YouTube link", submitLabel = "Add music", id,
}: {
  value: string;
  onChange: (text: string) => void;
  /** Called with a supported video link. */
  onSubmit: (url: string) => void;
  /** The URL being submitted right now, if any. */
  addingUrl?: string;
  disabled?: boolean;
  inputDisabled?: boolean;
  label?: string;
  submitLabel?: string;
  id?: string;
}) {
  const linkUrl = youTubeVideoUrl(value);
  function submit(e: FormEvent) {
    e.preventDefault();
    if (linkUrl && !disabled) onSubmit(linkUrl);
  }
  return (
    <div className="w-full flex flex-col gap-3">
      <form onSubmit={submit} className="flex flex-col sm:flex-row gap-3 w-full">
        <input
          id={id}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          aria-label={label}
          disabled={inputDisabled}
          placeholder={label}
          className="min-w-0 flex-1 p-3 rounded-lg bg-zinc-800 border border-zinc-700 placeholder-zinc-500 focus:outline-none focus:border-purple-500"
        />
        <button
          type="submit"
          disabled={disabled || !linkUrl}
          className="min-h-11 px-6 rounded-lg bg-[#5C172E] hover:bg-[#FF0048] disabled:opacity-50 transition font-bold"
        >
          {addingUrl && addingUrl === linkUrl ? "Adding…" : submitLabel}
        </button>
      </form>
      {!linkUrl && <SongSearch
        query={value}
        addingUrl={addingUrl}
        disabled={disabled}
        onAdd={onSubmit}
        onPasteLink={text => { onChange(text); const url = youTubeVideoUrl(text); if (url && !disabled) onSubmit(url); }}
      />}
    </div>
  );
}
