# CS 4398 Project

## Running with Docker

The whole stack (backend plus a Caddy reverse proxy that serves the frontend and `/api` from one origin) runs with:

```bash
cp .env.example .env   # fill in the Google OAuth values and a JWT secret
docker compose up --build
```

Then open [http://localhost:3000](http://localhost:3000) (bound to loopback only; set `HTTP_PORT` in `.env` to use a different port). The API docs are at `/scalar`. After a `git pull`, run `docker compose up --build` again; only changed crates are recompiled.

The SQLite database lives in the `backend-data` volume, so restarts and rebuilds keep accounts and lobbies. To reset it: `docker compose down -v`.

### Hosting

Set `SITE_ADDRESS` in `.env` to the public hostname (for example `crescend.example.com`) and change `GOOGLE_REDIRECT_URI` to `https://<hostname>/api/auth/google/callback`, registering that URI in the Google Cloud Console. Then start with the hosting override, which publishes ports 80/443 so Caddy can obtain the HTTPS certificate automatically:

```bash
docker compose -f docker-compose.yml -f docker-compose.hosting.yml up --build
```

Or uncomment `COMPOSE_FILE` in `.env` and keep using plain `docker compose up --build`. The `Caddyfile` already disables buffering for the lobby event stream.

### Hosting behind a TLS-terminating tunnel (Cloudflare Tunnel etc.)

When a tunnel or reverse proxy already terminates HTTPS, keep Caddy on plain HTTP and skip the hosting override. In `.env`:

```bash
GOOGLE_REDIRECT_URI=https://<hostname>/api/auth/google/callback
SITE_ADDRESS=:3000   # plain HTTP inside the box; the tunnel serves HTTPS
HTTP_PORT=8796       # loopback port the tunnel forwards to (any free port)
```

Point the tunnel at `http://127.0.0.1:<HTTP_PORT>`. The HTTPS redirect URI makes the backend issue `Secure` cookies and only trust `Origin: https://<hostname>`, so users must reach the app through the public hostname, not the loopback port. Redeploy with `git pull && docker compose up -d --build --force-recreate`.

The class demo runs this way at https://crescendio.backtitleai.com, with runbook in [docs/hosting-runbook.md](docs/hosting-runbook.md).

## Backend

### Prerequisites:

- Rust: [https://rust-lang.org/tools/install](https://rust-lang.org/tools/install)
- (Windows) C++ Build Tools: The Rust installer should automatically prompt to install. Otherwise follow: [https://rust-lang.github.io/rustup/installation/windows-msvc.html](https://rust-lang.github.io/rustup/installation/windows-msvc.html)

### Running

First cd to the backend directory

```bash
cd backend
```

Set the environment variables below, then run with

```bash
cargo run
```

or for release builds (optimization)

```bash
cargo run --release
```

### API documentation

With the backend running, open [Scalar](http://localhost:3000/scalar) to explore and try the existing endpoints.

See [browser authentication](docs/browser-auth.md) for login, access-token bootstrap, logout, deployment, and migration examples.

See [backend song search](docs/song-search-api.md) for catalog search, optional discovery configuration, and submission of returned links.

### Environment variables

For development, put these required variables in `backend/.env`:

| Variable | Value |
| --- | --- |
| `GOOGLE_CLIENT_ID` | Google OAuth client ID |
| `GOOGLE_CLIENT_SECRET` | Google OAuth client secret |
| `GOOGLE_REDIRECT_URI` | Registered callback URL (local: `http://localhost:3000/api/auth/google/callback`) |
| `JWT_SIGNING_SECRET` | Random signing secret, at least 32 bytes |
