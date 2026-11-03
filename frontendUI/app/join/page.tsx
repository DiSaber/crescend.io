"use client";

import { Suspense, useState, type FormEvent } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import SignInGate from "../components/SignInGate";
import { joinLobby } from "../lib/lobby";

function JoinSession({ initialCode }: { initialCode: string }) {
  const router = useRouter();
  const [code, setCode] = useState(initialCode);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  async function join(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      await joinLobby(code);
      router.push("/lobby/");
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  return (
    <form onSubmit={join} className="w-full max-w-md bg-zinc-900 p-8 rounded-xl border border-zinc-800">
      <h2 className="text-3xl font-bold text-center mb-6">Join a Session</h2>

      <div className="flex flex-col gap-4">
        <input
          type="text"
          value={code}
          onChange={(e) => setCode(e.target.value.toUpperCase())}
          placeholder="Session code (e.g. A1B2C3)"
          maxLength={12}
          autoFocus={!initialCode}
          className="w-full p-3 rounded-lg bg-zinc-800 border border-zinc-700 text-white text-center tracking-[0.3em] placeholder-zinc-500 placeholder:tracking-normal focus:outline-none focus:border-purple-500"
        />

        <button
          type="submit"
          autoFocus={!!initialCode}
          disabled={busy || code.trim().length === 0}
          className="w-full p-3 rounded-lg bg-purple-600 hover:bg-purple-700 disabled:opacity-50 transition font-semibold"
        >
          {busy ? "Joining…" : "Join Session"}
        </button>
        {error && <p className="text-red-400 text-center">{error}</p>}
      </div>
    </form>
  );
}

// A scanned QR code opens /join/?code=XXXXXX. The code stays in the return address so it
// survives the Google sign-in round trip.
function JoinFromLink() {
  const code = (useSearchParams().get("code") ?? "").toUpperCase().replace(/[^A-Z0-9]/g, "").slice(0, 12);
  return (
    <SignInGate returnTo={code ? `/join/?code=${code}` : "/join/"}>
      <JoinSession initialCode={code} />
    </SignInGate>
  );
}

export default function JoinSessionPage() {
  return (
    <div className="flex flex-col items-center justify-center min-h-screen bg-gradient-to-b from-black to-[#1a0b29] text-white px-6">
      <Suspense>
        <JoinFromLink />
      </Suspense>
    </div>
  );
}
