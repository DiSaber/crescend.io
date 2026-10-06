// Browser auth coordinator, following docs/browser-auth.md.
// The access token lives only in memory; the refresh cookie is HttpOnly and
// rotated by the backend, so refreshes are serialized across tabs with a Web Lock.

let accessToken: string | undefined;
let refreshing: Promise<string | undefined> | undefined;

async function withAuthLock<T>(fn: () => Promise<T>): Promise<T> {
  if (typeof navigator !== "undefined" && navigator.locks) {
    return navigator.locks.request("crescend-auth", fn);
  }
  return fn();
}

/** Exchange the refresh cookie for a new access token. Resolves undefined when signed out. */
export function refresh(): Promise<string | undefined> {
  refreshing ??= withAuthLock(async () => {
    const response = await fetch("/api/auth/refresh", {
      method: "POST",
      credentials: "same-origin",
      headers: { "X-CSRF-Protection": "1" },
    });
    if (response.status === 401) {
      accessToken = undefined;
      return undefined;
    }
    if (!response.ok) throw new Error(`Refresh failed: ${response.status}`);
    const token = await response.json();
    accessToken = token.access_token;
    return accessToken;
  }).finally(() => {
    refreshing = undefined;
  });
  return refreshing;
}

/** Current access token, refreshing once if none is held yet. */
export async function getToken(): Promise<string | undefined> {
  return accessToken ?? refresh();
}

export function login(returnTo: string) {
  window.location.assign(
    "/api/auth/google/start?" + new URLSearchParams({ return_to: returnTo }),
  );
}

export async function logout() {
  accessToken = undefined;
  await withAuthLock(() =>
    fetch("/api/auth/logout", {
      method: "POST",
      credentials: "same-origin",
      headers: { "X-CSRF-Protection": "1" },
    }),
  );
}

export class SignedOutError extends Error {}

/** fetch with the bearer token; retries once after a 401 with a fresh token. */
export async function apiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const send = (token: string) =>
    fetch(path, {
      ...init,
      cache: "no-store",
      headers: { ...init.headers, Authorization: `Bearer ${token}` },
    });
  let token = await getToken();
  if (!token) throw new SignedOutError();
  let response = await send(token);
  if (response.status === 401) {
    token = await refresh();
    if (!token) throw new SignedOutError();
    response = await send(token);
  }
  return response;
}
