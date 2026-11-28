# Running Airobot on a server

One small server (1–2 GB of memory is enough to start) running three
containers: Airobot, PostgreSQL and Caddy, which gets the HTTPS certificate by
itself. **Nothing is built on the server.** GitHub builds the Docker image when
a release is tagged and publishes it to `ghcr.io/azavibe/airobot`; the server
only pulls it.

## Quick install

On a fresh Debian or Ubuntu server, logged in as root (an SSH app on your
phone, such as Termius, or your provider's browser console works too):

```sh
curl -fsSL https://raw.githubusercontent.com/azavibe/airo-bot/main/deploy/install.sh | bash
```

It does steps 1, 3 and 4 below for you: Docker, the firewall (SSH, 80 and 443;
it keeps whatever port SSH uses open), the files in `/opt/airobot`, and the
settings. It makes the secrets on the server, where they stay (`.env`,
readable by root only), and asks only for the domain and your mail service's
SMTP settings. Point the domain at the server first (step 2). Running it again
keeps the settings and updates to the newest release.

You still do the rest of step 1 yourself: change the root password and switch
to an SSH key.

## 1. Prepare the server

Do this once, as root, on a fresh Debian or Ubuntu server.

```sh
# Change the root password you were emailed (it was sent in clear text), then
# log in with an SSH key instead: from your computer, `ssh-copy-id root@<server>`.
passwd

# Docker: follow https://docs.docker.com/engine/install/ for your system.

# Firewall: SSH, HTTP and HTTPS only.
ufw allow 22/tcp && ufw allow 80/tcp && ufw allow 443/tcp && ufw allow 443/udp
ufw enable
```

Once the key works, set `PasswordAuthentication no` in `/etc/ssh/sshd_config`
and `systemctl restart ssh`, so nobody can guess a password.

PostgreSQL is never published to the internet: only Caddy has ports (80, 443).

## 2. Point the domain at it

At your domain's DNS provider (Spaceship: Domains → the domain → DNS records),
make these records, and delete any others for `@` and `www` (parking pages,
old servers):

| Type | Host | Value |
|---|---|---|
| A | `@` | the server's IPv4 address |
| A | `www` | the server's IPv4 address |

Don't add an `AAAA` (IPv6) record yet. Docker doesn't pass IPv6 visitors'
addresses through to the app, so every IPv6 visitor would look like one person
to the per-address limits. Add it only after setting Docker up for IPv6.

Check from your computer: `dig +short <your domain>` shows the server's
address. Caddy can't get a certificate until it does.

## 3. Configure

```sh
git clone https://github.com/azavibe/airo-bot.git
cd airo-bot/deploy
cp env.example .env
chmod 600 .env
```

Fill in `.env`; every setting is explained in it. The ones the server refuses
to start without:

- `DOMAIN` (no `https://`), `POSTGRES_PASSWORD` (`openssl rand -hex 24`).
- `PROVIDER_KEY_SECRET` (`openssl rand -hex 32`). **It can never change** once
  people have saved AI keys or connected Google, Stripe or Shopify. Back it up
  somewhere safe, apart from the server.
- `VAPID_PRIVATE_KEY` (the command is in the file).
- `SMTP_*`: sign-in links are sent by email, so a mail service is required
  (Resend, Postmark, Amazon SES or your own SMTP server). Set up its SPF and
  DKIM records for the sending domain at your DNS provider, or the links land
  in spam.

## 4. Start

```sh
docker compose pull          # needs a published release, see "Releases" below
docker run --rm --env-file .env -v "$PWD/Caddyfile:/etc/caddy/Caddyfile:ro" caddy:2 caddy validate --config /etc/caddy/Caddyfile
docker compose up -d
docker compose logs -f app   # Ctrl-C to leave
```

If the app stops right away, its last log line says which setting is missing
or wrong. When it's up, open `https://<your domain>/health` (it says `ok`),
then the site itself. **The first account that signs in is the platform's
admin**, so sign in yourself before telling anyone the address.

If the certificate isn't issued, check `docker compose logs caddy`: nearly
always the DNS record hasn't spread yet, or port 80/443 is blocked.

## Secure connection

What you get without extra setup:

- **HTTPS everywhere.** Caddy gets a free Let's Encrypt certificate for the
  domain, renews it by itself, speaks only modern TLS (1.2 and 1.3), and
  redirects plain HTTP to HTTPS.
- **HSTS.** Browsers that have visited once refuse to use plain HTTP for the
  site for a year, so nobody can be downgraded on a café network.
- **Secure sign-in cookies.** With `APP_BASE_URL` on `https://` (the production
  setup enforces this) the session cookie is sent over HTTPS only, and the
  server refuses to start otherwise.
- **Secrets encrypted at rest** (`PROVIDER_KEY_SECRET`), sign-in secrets stored
  only as hashes, and the database never reachable from outside the server.

Check it after the first start: `curl -sI https://<your domain>` shows
`HTTP/2 200` and a `strict-transport-security` line, and
`curl -sI http://<your domain>` answers with a redirect to `https://`. For a
fuller report, run the domain through <https://www.ssllabs.com/ssltest/>.

Connecting to the server itself is secured by the SSH key and firewall in
step 1; don't leave password login on.

## Releases

Maintainers release by pushing a tag, `v<version>` (see `CHANGELOG.md`); GitHub
Actions then builds and publishes the image. If `docker compose pull` says
`denied` or `not found`, either no release has been published yet, or the
package is private: make it public in GitHub (the repository's Packages →
`airobot` → Package settings), or run `docker login ghcr.io` on the server
with a token that has `read:packages`.

## Updating

```sh
cd airo-bot/deploy
nano .env                    # AIROBOT_VERSION=<the new version>  (or leave "latest")
docker compose pull
docker compose up -d
```

The database is updated automatically when the new version starts. To go back,
set the previous `AIROBOT_VERSION` and run the same two commands. A version
that changed the database can't read it in reverse, so after an update that
did, go back by restoring the backup taken before it.

## Backups

The database is the only thing that matters (plus your `.env`, kept apart).

```sh
mkdir -p ~/backups
docker compose exec -T postgres pg_dump -U airobot_user airobot | gzip > ~/backups/airobot-$(date +%F).sql.gz
```

To run it every night and keep two weeks, add this with `crontab -e` (adjust the
path to where you cloned it):

```cron
15 3 * * * cd /root/airo-bot/deploy && docker compose exec -T postgres pg_dump -U airobot_user airobot | gzip > /root/backups/airobot-$(date +\%F).sql.gz && find /root/backups -name 'airobot-*.sql.gz' -mtime +14 -delete
```

Copy the backups off the server too (your provider's backup option, or `rsync`
to another machine); a backup on the same disk doesn't survive losing the server.

To restore into an empty database: stop the app (`docker compose stop app`),
then `gunzip -c ~/backups/airobot-<date>.sql.gz | docker compose exec -T postgres psql -U airobot_user airobot`
and start it again.

## How it's wired

- **Caddy** (`Caddyfile`) serves `https://<domain>`, sends `www.` to it, and
  forwards to the app. It has a fixed address (172.30.0.2) on the Compose
  network, which the app lists in `TRUSTED_PROXY_IPS`: the visitor address
  Caddy reports is the only one the app believes for its per-address limits.
- **The app** runs with `APP_ENV=production`, so it refuses to start with
  development shortcuts or missing secrets.
- **Data** lives in Docker volumes: `airobot_postgres_data` (the database) and
  `caddy_data` (certificates; keep them, Let's Encrypt limits how often it will
  issue new ones).
