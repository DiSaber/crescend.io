# Hosted demo runbook (crescendio.backtitleai.com)

The class demo runs on Daniel's homelab services VM behind the existing Cloudflare Tunnel. Cost is $0 and there are no router port forwards.

| What | Where |
|---|---|
| Public URL | https://crescendio.backtitleai.com |
| Checkout | `/srv/compose/crescendio` on the services VM (git clone of `DiSaber/crescend.io`, branch `main`) |
| Compose project | `crescendio` (containers `crescendio-backend-1`, `crescendio-caddy-1`) |
| Caddy | plain HTTP on `127.0.0.1:8796` (`SITE_ADDRESS=:3000`, `HTTP_PORT=8796`) |
| Secrets | `/srv/compose/crescendio/.env`, mode 600, server only, never committed |
| Data | docker volume `crescendio_backend-data` (SQLite at `/data` in the backend), kept across rebuilds |
| Tunnel | `/srv/compose/.cloudflared/config.yml`, ingress rule `crescendio.backtitleai.com -> http://127.0.0.1:8796`, container `cloudflared` |
| Google OAuth | redirect URI `https://crescendio.backtitleai.com/api/auth/google/callback` must be registered on the OAuth client |

## Deploy / redeploy

```bash
ssh services-vm
cd /srv/compose/crescendio
git pull
docker compose up -d --build --force-recreate
docker compose ps        # backend should be "healthy"
curl -s -o /dev/null -w '%{http_code}\n' https://crescendio.backtitleai.com/
```

## Logs

```bash
cd /srv/compose/crescendio
docker compose logs -f --tail=100 backend
docker compose logs -f --tail=100 caddy
docker logs --tail=50 cloudflared     # tunnel side
```

## Rollback

```bash
cd /srv/compose/crescendio
git log --oneline -10
git checkout <good-sha>               # detached HEAD on the last good commit
docker compose up -d --build --force-recreate
# when main is fixed:
git checkout main && git pull && docker compose up -d --build --force-recreate
```

Database: rollbacks keep the volume. A schema change that an older build can't read needs a backup taken first:
`docker run --rm -v crescendio_backend-data:/d -v $PWD:/b alpine tar czf /b/backend-data-$(date +%F).tgz -C /d .`

## Take it offline

- **Stop public access but keep it running:** remove the `crescendio.backtitleai.com` ingress rule from `/srv/compose/.cloudflared/config.yml` (back up first), then `docker restart cloudflared`. The hostname then gets the catch-all 404. Optionally delete the `crescendio` CNAME in the Cloudflare dashboard (DNS → Records).
- **Stop the app:** `cd /srv/compose/crescendio && docker compose down`. This keeps the data volume. `down -v` wipes accounts and lobbies.
