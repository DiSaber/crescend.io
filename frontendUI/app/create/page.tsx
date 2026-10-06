"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import SignInGate from "../components/SignInGate";
import { createLobby } from "../lib/lobby";

function CreateSession() {
  const router = useRouter();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  async function create() {
    setBusy(true);
    setError(undefined);
    try {
      await createLobby();
      router.push("/lobby/");
    } catch (e) {
      setError((e as Error).message);
      setBusy(false);
    }
  }

  return (
    <div className="w-full max-w-md bg-zinc-900 p-8 rounded-xl border border-zinc-800">
      <h2 className="text-3xl font-bold text-center mb-6">Create a Session</h2>

      <div className="flex flex-col gap-4">
        <p className="text-zinc-400 text-center">
          You&apos;ll be the host. Share the six-character code so friends can join.
        </p>

        <button
          onClick={create}
          disabled={busy}
          className="w-full p-3 rounded-lg bg-purple-600 hover:bg-purple-700 disabled:opacity-50 transition font-semibold"
        >
          {busy ? "Creating…" : "Create Session"}
        </button>
        {error && <p className="text-red-400 text-center">{error}</p>}
      </div>
    </div>
  );
}

export default function CreateSessionPage() {
  return (
    <div className="flex flex-col items-center justify-center min-h-screen text-white px-6">
      <SignInGate returnTo="/create/">
        <CreateSession />
      </SignInGate>
    </div>
  );
}
