"use client";

import { useEffect, useState, type ReactNode } from "react";
import { login, refresh } from "../lib/auth";

type Status = "checking" | "signed-in" | "signed-out" | "error";

/** Renders children only after the refresh cookie yields an access token. */
export default function SignInGate({ returnTo, children }: { returnTo: string; children: ReactNode }) {
  const [status, setStatus] = useState<Status>("checking");

  useEffect(() => {
    refresh()
      .then((token) => setStatus(token ? "signed-in" : "signed-out"))
      .catch(() => setStatus("error"));
  }, []);

  if (status === "signed-in") return <>{children}</>;

  return (
    <div className="w-full max-w-md bg-zinc-900 p-8 rounded-xl border border-zinc-800 text-center">
      {status === "checking" && <p className="text-zinc-400">Checking sign-in…</p>}
      {status === "error" && <p className="text-red-400 mb-4">Couldn&apos;t reach the server. Is the backend running?</p>}
      {status !== "checking" && (
        <>
          <p className="text-zinc-300 mb-6">Sign in to create or join a session.</p>
          <button
            onClick={() => login(returnTo)}
            className="w-full p-3 rounded-lg bg-purple-600 hover:bg-purple-700 transition font-semibold"
          >
            Sign in with Google
          </button>
        </>
      )}
    </div>
  );
}
