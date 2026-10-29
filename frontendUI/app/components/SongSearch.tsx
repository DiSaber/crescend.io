"use client";

import { useEffect, useState } from "react";
import YouTubeSearchFallback from "./YouTubeSearchFallback";
import { searchSongs, type SongResult } from "../lib/songs";
import { youTubeSearchUrl } from "../lib/youtube";

const DEBOUNCE_MS = 250;
const MIN_CHARS = 2;

/**
 * Search-as-you-type over the song catalog (#41) for text that isn't a video link.
 * Add hands the result's watch URL to the lobby's normal submit flow, exactly like a
 * pasted link. No matches (or search unavailable) falls back to searching YouTube by hand.
 */
export default function SongSearch({
  query,
  addingUrl,
  onAdd,
  onPasteLink,
  onMoreResults,
}: {
  query: string;
  /** The URL being submitted right now, if any; disables every Add button meanwhile. */
  addingUrl?: string;
  onAdd: (url: string) => void;
  onPasteLink?: (text: string) => void;
  /**
   * Seam for the YouTube Data API fallback (#41 part 2): when provided, a "Search YouTube
   * for more" button calls it. Not wired yet; the catalog is the only source today.
   */
  onMoreResults?: () => void;
}) {
  const q = query.trim();
  // Results are tagged with the query they answer, so a slow reply never shows under
  // newer text; null means the search failed.
  const [found, setFound] = useState<{ q: string; results: SongResult[] | null }>({ q: "", results: [] });

  useEffect(() => {
    if (q.length < MIN_CHARS) return;
    const controller = new AbortController();
    const timer = setTimeout(() => {
      searchSongs(q, controller.signal)
        .then((response) => setFound({ q, results: response.results }))
        .catch(() => {
          if (!controller.signal.aborted) setFound({ q, results: null });
        });
    }, DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [q]);

  if (q.length < MIN_CHARS) return null;
  const settled = found.q === q;
  const results = found.results;

  if (settled && !results?.length) {
    return (
      <div className="w-full flex flex-col gap-2">
        <p className="text-sm text-zinc-400">
          {results ? <>No saved songs match &ldquo;{q}&rdquo;.</> : "Song search is unavailable right now."}
        </p>
        <YouTubeSearchFallback query={q} onPasteLink={onPasteLink} />
      </div>
    );
  }
  if (!results?.length) {
    return <p className="text-sm text-zinc-500" aria-live="polite">Searching…</p>;
  }

  return (
    <div className={`w-full flex flex-col gap-2 transition-opacity ${settled ? "" : "opacity-60"}`} aria-busy={!settled}>
      <ul className="flex flex-col gap-2" aria-label="Song search results">
        {results.map((song) => (
          <li key={song.video_id} className="flex items-center gap-3 rounded-lg bg-zinc-800 p-2">
            <img src={song.thumbnail_url} alt="" className="h-12 w-20 shrink-0 rounded object-cover" />
            <div className="min-w-0 flex-1">
              <p className="truncate text-sm font-medium">{song.title}</p>
              <p className="truncate text-xs text-zinc-400">{song.artist}</p>
            </div>
            <button
              type="button"
              onClick={() => onAdd(song.url)}
              disabled={!!addingUrl}
              aria-label={`Add ${song.title}`}
              className="min-h-11 min-w-11 shrink-0 px-4 rounded-lg bg-[#5C172E] hover:bg-[#FF0048] disabled:opacity-50 transition font-semibold"
            >
              {addingUrl === song.url ? "Adding…" : "Add"}
            </button>
          </li>
        ))}
      </ul>
      {onMoreResults ? (
        <button
          type="button"
          onClick={onMoreResults}
          className="min-h-11 px-4 rounded-lg bg-zinc-800 border border-zinc-700 hover:border-[#FF0048] transition font-semibold"
        >
          Search YouTube for more
        </button>
      ) : (
        <p className="text-sm text-zinc-400">
          Not listed?{" "}
          <a
            href={youTubeSearchUrl(q)}
            target="_blank"
            rel="noopener noreferrer"
            className="py-3.5 underline hover:text-white"
          >
            Search YouTube
          </a>{" "}
          and paste the video&apos;s link above.
        </p>
      )}
    </div>
  );
}
