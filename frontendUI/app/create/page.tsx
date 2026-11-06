"use client";

import { useEffect, useState, type FormEvent } from "react";
import { useRouter } from "next/navigation";
import SignInGate from "../components/SignInGate";
import { account, login, type Account } from "../lib/auth";
import { createLobby, nicknameProblem, NICKNAME_MAX } from "../lib/lobby";

// Hosting stays tied to a Google account; guests are sent to Google sign-in.
function CreateSession() {
  const router = useRouter();
  const [me, setMe] = useState<Account | null>();
  const [nickname, setNickname] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let active = true;
    account()
      .then((value) => {
        if (!active) return;
        setMe(value ?? null);
        if (value?.last_nickname) setNickname((current) => current || value.last_nickname!);
      })
      .catch(() => { if (active) setMe(null); });
    return () => { active = false; };
  }, []);

  async function create(e: FormEvent) {
    e.preventDefault();
    const problem = nicknameProblem(nickname);
    if (problem) { setError(problem); return; }
    setBusy(true);
    setError(undefined);
    try {
      await createLobby(nickname.trim());
      router.push("/lobby/");
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  if (me === undefined) return <p className="text-zinc-400">Checking sign-in…</p>;
  if (me?.guest) return (
    <div className="w-full max-w-md bg-zinc-900 p-6 sm:p-8 rounded-xl border border-zinc-800 text-center">
      <h2 className="text-3xl font-bold mb-4">Create a Session</h2>
      <p className="text-zinc-300 mb-6">You&apos;re joined as a guest. Hosting a session needs a Google account.</p>
      <button
        onClick={() => login("/create/")}
        className="w-full min-h-12 p-3 rounded-lg bg-purple-600 hover:bg-purple-700 transition font-semibold"
      >
        Sign in with Google
      </button>
    </div>
  );

  return (
    <form onSubmit={create} className="w-full max-w-md bg-zinc-900 p-6 sm:p-8 rounded-xl border border-zinc-800">
      <h2 className="text-3xl font-bold text-center mb-6">Create a Session</h2>

      <div className="flex flex-col gap-4">
        <p className="text-zinc-400 text-center">
          You&apos;ll be the host. Share the six-character code so friends can join.
        </p>
        <label className="flex flex-col gap-1 text-sm text-zinc-400">
          Your nickname
          <input
            type="text"
            name="nickname"
            value={nickname}
            onChange={(e) => setNickname(e.target.value)}
            placeholder="What should everyone call you?"
            maxLength={NICKNAME_MAX * 2}
            autoComplete="nickname"
            autoFocus
            className="w-full p-3 rounded-lg bg-zinc-800 border border-zinc-700 text-white text-lg placeholder-zinc-500 focus:outline-none focus:border-purple-500"
          />
        </label>

        <button
          type="submit"
          disabled={busy || nickname.trim().length === 0}
          className="w-full min-h-12 p-3 rounded-lg bg-purple-600 hover:bg-purple-700 disabled:opacity-50 transition font-semibold"
        >
          {busy ? "Creating…" : "Create Session"}
        </button>
        {error && <p role="alert" className="text-red-400 text-center">{error}</p>}
      </div>
    </form>
  );
}

export default function CreateSessionPage() {
  return (
    <div className="flex flex-col items-center justify-center min-h-screen bg-gradient-to-b from-black to-[#1a0b29] text-white px-4">
      <SignInGate returnTo="/create/" message="Sign in with Google to host a session.">
        <CreateSession />
      </SignInGate>
    </div>
  );
}
