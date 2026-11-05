"use client";

import { useEffect, useState } from "react";

export default function PartyTimer() {
  const [timeLeft, setTimeLeft] = useState(60);
  const [isRunning, setIsRunning] = useState(false);

  useEffect(() => {
    if (!isRunning || timeLeft <= 0) return;

    const timer = window.setInterval(() => {
      setTimeLeft((previous) => {
        if (previous <= 1) {
          setIsRunning(false);
          return 0;
        }
        return previous - 1;
      });
    }, 1000);

    return () => window.clearInterval(timer);
  }, [isRunning, timeLeft]);

  return (
    <div className="mt-3 flex flex-col items-center gap-2">
      <h2 className="font-mono text-lg font-bold text-red-400">
        Party timer {String(Math.floor(timeLeft / 60)).padStart(2, "0")}:
        {String(timeLeft % 60).padStart(2, "0")}
      </h2>
      <div className="flex gap-2">
        <button
          onClick={() => setIsRunning(true)}
          disabled={isRunning || timeLeft === 0}
          className="rounded-lg bg-red-600 px-3 py-1 text-sm font-semibold hover:bg-red-700 disabled:opacity-50"
        >
          Start
        </button>
        <button
          onClick={() => {
            setIsRunning(false);
            setTimeLeft(60);
          }}
          className="rounded-lg bg-zinc-700 px-3 py-1 text-sm font-semibold hover:bg-zinc-600"
        >
          Reset
        </button>
      </div>
    </div>
  );
}
