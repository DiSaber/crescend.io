"use client";

import { useSyncExternalStore } from "react";
import { youTubeSearchUrl } from "../lib/youtube";

const noSubscribe = () => () => {};
const canReadClipboard = () => typeof navigator !== "undefined" && !!navigator.clipboard?.readText;

/**
 * For text that isn't a video link: search YouTube for it in a new tab (the YouTube app
 * on phones), then come back and paste the video's link. The catalog search (#41) can
 * render this under "no results".
 */
export default function YouTubeSearchFallback({
  query,
  onPasteLink,
}: {
  query: string;
  /** Called with the clipboard text after a tap on "Paste link". Omit to hide the button. */
  onPasteLink?: (text: string) => void;
}) {
  // False during the static prerender, so the button only appears where the API exists.
  const clipboard = useSyncExternalStore(noSubscribe, canReadClipboard, () => false);
  const text = query.trim();
  if (!text) return null;

  async function paste() {
    try {
      const pasted = (await navigator.clipboard.readText()).trim();
      if (pasted) onPasteLink?.(pasted);
    } catch {
      // Permission denied or nothing readable: leave the input alone.
    }
  }

  return (
    <div className="w-full flex flex-col gap-2">
      <div className="flex flex-col sm:flex-row gap-3">
        <a
          href={youTubeSearchUrl(text)}
          target="_blank"
          rel="noopener noreferrer"
          className="flex-1 min-w-0 min-h-11 flex items-center justify-center gap-2 px-4 rounded-lg bg-zinc-800 border border-zinc-700 hover:border-[#FF0048] transition font-semibold"
        >
          <span className="truncate">Search YouTube for &ldquo;{text}&rdquo;</span>
        </a>
        {clipboard && onPasteLink && (
          <button
            type="button"
            onClick={paste}
            className="min-h-11 px-6 rounded-lg bg-zinc-800 border border-zinc-700 hover:border-purple-500 transition font-semibold"
          >
            Paste link
          </button>
        )}
      </div>
      <p className="text-sm text-zinc-400">Copy the video&apos;s link, come back and paste it here.</p>
    </div>
  );
}
