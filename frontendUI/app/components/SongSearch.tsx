"use client";

import { useEffect, useState } from "react";
import YouTubeSearchFallback from "./YouTubeSearchFallback";
import { searchSongs, type SongResult } from "../lib/songs";
import { youTubeSearchUrl } from "../lib/youtube";

const DEBOUNCE_MS = 250;
const MIN_CHARS = 2;

type Found = { q: string; results: SongResult[] } | { q: string; error: true };

/**
 * Search-as-you-type for text that isn't a video link, on GET /api/songs/search (#51).
 * Add hands the result's watch URL to the lobby's normal submit flow, exactly like a
 * pasted link. No results, or search unavailable, falls back to searching YouTube by
 * hand (#47).
 */
export default function SongSearch({
  query,
  addingUrl,
  onAdd,
  onPasteLink,
}: {
  query: string;
  /** The URL being submitted right now, if any; disables every Add button meanwhile. */
  addingUrl?: string;
  onAdd: (url: string) => void;
  onPasteLink?: (text: string) => void;
}) {
  const q = query.trim();
  // Responses are tagged with the text they answer, so a slow reply never shows under
  // newer text.
  const [found, setFound] = useState<Found>({ q: "", results: [] });

  useEffect(() => {
    if (q.length < MIN_CHARS) return;
    const controller = new AbortController();
    const timer = setTimeout(() => {
      searchSongs(q, controller.signal)
        .then((results) => setFound({ q, results }))
        .catch(() => {
          if (!controller.signal.aborted) setFound({ q, error: true });
        });
    }, DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [q]);

  if (q.length < MIN_CHARS) return null;
  const settled = found.q === q;
  // While a new search runs, keep showing (dimmed) only results for text this one extends
  // or trims, never results for an unrelated earlier search.
  const related = q.startsWith(found.q) || found.q.startsWith(q);
  const results = "error" in found || (!settled && !related) ? null : found.results;

  if (settled && !results?.length) {
    return (
      <div className="w-full flex flex-col gap-2">
        <p className="text-sm text-zinc-400">
          {"error" in found ? "Song search is unavailable right now." : `No songs match “${q}”.`}
        </p>
        <YouTubeSearchFallback query={q} onPasteLink={onPasteLink} />
      </div>
    );
  }
  if (!results?.length) {
    return <p className="text-sm text-zinc-500" aria-live="polite">Searching…</p>;
  }

  return (
    <div className="w-full flex flex-col gap-2">
      <ul
        className={`flex flex-col gap-2 transition-opacity ${settled ? "" : "opacity-60"}`}
        aria-busy={!settled}
        aria-label="Song search results"
      >
        {results.map((song) => (
          <li key={`${song.source}-${song.video_id}`} className="flex items-center gap-3 rounded-lg bg-zinc-800 p-2">
            {song.thumbnail_url ? (
              <img src={song.thumbnail_url} alt="" className="h-12 w-20 shrink-0 rounded object-cover" />
            ) : (
              <div className="h-12 w-20 shrink-0 rounded bg-zinc-700" aria-hidden="true" />
            )}
            <div className="min-w-0 flex-1">
              <p className="line-clamp-2 text-sm font-medium">{song.title}</p>
              <p className="truncate text-xs text-zinc-400">{song.artist}</p>
            </div>
            <button
              type="button"
              onClick={() => onAdd(song.watch_url)}
              disabled={!!addingUrl}
              aria-label={`Add ${song.title}`}
              className="min-h-11 min-w-11 shrink-0 px-4 rounded-lg bg-[#5C172E] hover:bg-[#FF0048] disabled:opacity-50 transition font-semibold"
            >
              {addingUrl === song.watch_url ? "Adding…" : "Add"}
            </button>
          </li>
        ))}
      </ul>
      <p className="text-sm text-zinc-400">
        Not listed?{" "}
        <a href={youTubeSearchUrl(q)} target="_blank" rel="noopener noreferrer" className="inline-block py-3 underline hover:text-white">
          Search YouTube
        </a>{" "}
        and paste the video&apos;s link above.
      </p>
    </div>
  );
}
