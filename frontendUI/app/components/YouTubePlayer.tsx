import { useEffect, useRef, useState } from "react";
import { playbackEvents } from "../lib/playback-events";

type Player = { destroy: () => void; playVideo: () => void };
type YouTubeApi = {
  Player: new (node: HTMLElement, options: {
    videoId: string; width: string; height: string; playerVars: Record<string, string | number>;
    events: {
      onReady: (event: { target: Player }) => void;
      onStateChange: (event: { data: number }) => void;
      onError: (event: { data: number }) => void;
      onAutoplayBlocked: () => void;
    };
  }) => Player;
};
declare global { interface Window { YT?: YouTubeApi; onYouTubeIframeAPIReady?: () => void; } }
let loading: Promise<YouTubeApi> | undefined;
function loadApi() {
  if (window.YT?.Player) return Promise.resolve(window.YT);
  if (loading) return loading;
  const promise = new Promise<YouTubeApi>((resolve, reject) => {
    const previous = window.onYouTubeIframeAPIReady;
    const script = document.createElement("script");
    script.src = "https://www.youtube.com/iframe_api"; script.async = true;
    const cleanup = () => { clearTimeout(timer); window.onYouTubeIframeAPIReady = previous; };
    const fail = () => { cleanup(); script.remove(); reject(new Error("Could not load the YouTube player. Retry when connected.")); };
    const timer = setTimeout(fail, 20000);
    window.onYouTubeIframeAPIReady = () => {
      cleanup(); previous?.();
      if (window.YT?.Player) resolve(window.YT); else fail();
    };
    script.onerror = fail; document.head.appendChild(script);
  });
  loading = promise;
  void promise.catch(() => { if (loading === promise) loading = undefined; });
  return promise;
}

/**
 * A normal, visible player (never covered, shrunk below 200x200 or hidden; see
 * docs/game-modes.md). Remount per occurrence, including consecutive occurrences of
 * the same video: `occurrence` is a queue entry ID or a game play_seq.
 */
export default function YouTubePlayer({ videoId, occurrence, start = 0, disabled, onEnded, onSkipError }: {
  videoId: string; occurrence: string; start?: number; disabled: boolean; onEnded: () => void; onSkipError: () => void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const player = useRef<Player | undefined>(undefined);
  const callbacks = useRef({ onEnded });
  const [retry, setRetry] = useState(0);
  const [error, setError] = useState<string>();
  useEffect(() => { callbacks.current = { onEnded }; }, [onEnded]);
  useEffect(() => {
    let cancelled = false; let instance: Player | undefined;
    const events = playbackEvents({
      ended: () => callbacks.current.onEnded(), error: setError, playing: () => setError(undefined),
    });
    loadApi().then(api => {
      if (cancelled || !container.current) return;
      // YouTube replaces its target node; keep React's container intact for cleanup.
      const target = document.createElement("div"); container.current.replaceChildren(target);
      instance = new api.Player(target, {
        videoId, width: "100%", height: "100%",
        playerVars: { autoplay: 1, enablejsapi: 1, playsinline: 1, origin: window.location.origin, ...(start > 0 ? { start } : {}) },
        events: {
          onReady: () => {},
          onStateChange: events.state,
          onError: events.error,
          onAutoplayBlocked: events.blocked,
        },
      });
      player.current = instance;
    }).catch(e => { if (!cancelled) setError(e.message); });
    return () => { cancelled = true; events.dispose(); player.current = undefined; instance?.destroy(); };
  }, [occurrence, videoId, start, retry]);
  return (
    <div>
      <div className="relative w-full aspect-video min-h-[200px] rounded-lg overflow-hidden bg-black [&_iframe]:absolute [&_iframe]:inset-0 [&_iframe]:size-full" ref={container} />
      <div className="mt-2 flex flex-wrap items-center gap-2 text-sm">
        <button type="button" disabled={disabled} onClick={() => player.current?.playVideo()} className="min-h-11 rounded bg-zinc-700 px-4 disabled:opacity-50">Play</button>
        {error && <>
          <p role="alert" className="text-amber-300 basis-full">{error}</p>
          <button type="button" disabled={disabled} onClick={() => { setError(undefined); setRetry(n => n + 1); }} className="min-h-11 px-3 underline disabled:opacity-50">Retry player</button>
          <button type="button" disabled={disabled} onClick={onSkipError} className="min-h-11 px-3 text-red-300 underline disabled:opacity-50">Skip unplayable song</button>
        </>}
      </div>
    </div>
  );
}
