#!/usr/bin/env bash
# `ask` sets its variable with printf -v, which ShellCheck can't follow.
# shellcheck disable=SC2154
# Installs Airobot on a fresh Debian or Ubuntu server, as root:
#
#   curl -fsSL https://raw.githubusercontent.com/azavibe/airo-bot/main/deploy/install.sh | bash
#
# It installs Docker and a firewall, downloads the deploy files into
# /opt/airobot, makes the secrets (on the server only: they never leave it),
# asks for the domain and the mail service, and starts Airobot behind
# Caddy with HTTPS. Running it again keeps the existing settings and
# secrets, and updates to the newest release.
set -euo pipefail

FILES="https://raw.githubusercontent.com/azavibe/airo-bot/main/deploy"
DIR=/opt/airobot

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\n\033[31mStopped: %s\033[0m\n' "$*" >&2; exit 1; }
# Questions read from the terminal, since the script itself arrives on stdin.
ask() { # ask VAR "Question" [default]
	local answer
	read -r -p "$2${3:+ [$3]}: " answer </dev/tty || true
	printf -v "$1" '%s' "${answer:-${3:-}}"
}
ask_secret() { # ask_secret VAR "Question"
	local answer
	read -r -s -p "$2: " answer </dev/tty || true
	echo
	printf -v "$1" '%s' "$answer"
}

[ "$(id -u)" -eq 0 ] || fail "run it as root (or with sudo)."
# shellcheck source=/dev/null
[ -r /etc/os-release ] && . /etc/os-release
case "${ID:-}" in
	debian | ubuntu) ;;
	*) fail "this script is for Debian or Ubuntu; see deploy/README.md for other systems." ;;
esac
[ -e /dev/tty ] || fail "run it from a terminal (it asks a few questions)."

say "1/6  System packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq ca-certificates curl openssl ufw >/dev/null

say "2/6  Docker"
if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
	echo "Docker is already installed."
else
	# Docker's own installer (https://docs.docker.com/engine/install/).
	curl -fsSL https://get.docker.com | sh
fi
systemctl enable --now docker >/dev/null 2>&1 || true

say "3/6  Firewall: SSH, HTTP and HTTPS only"
# Keep every port SSH listens on open, so this never locks you out.
ssh_ports=$(ss -tlnpH 2>/dev/null | awk '/sshd/ {n=split($4, a, ":"); print a[n]}' | sort -u)
for port in 22 $ssh_ports; do
	ufw allow "$port/tcp" >/dev/null
done
ufw allow 80/tcp >/dev/null
ufw allow 443/tcp >/dev/null
ufw allow 443/udp >/dev/null
ufw --force enable >/dev/null
# shellcheck disable=SC2086 # the port list is meant to split
echo "Open: SSH ($(printf '%s\n' 22 $ssh_ports | sort -u | tr '\n' ' ')), 80, 443."

say "4/6  Airobot's files in $DIR"
mkdir -p "$DIR"
cd "$DIR"
for f in docker-compose.yml Caddyfile env.example README.md; do
	curl -fsSL "$FILES/$f" -o "$f"
done

say "5/6  Settings"
if [ -f .env ]; then
	echo "Keeping the existing settings in $DIR/.env (and their secrets)."
else
	ask domain "The site's address, without https://" "airo-bot.com"
	domain=${domain#https://}
	domain=${domain#http://}
	domain=${domain%%/*}
	echo
	echo "Sign-in links are sent by email, so Airobot needs a mail service"
	echo "(Resend, Postmark, Amazon SES, ...). Its SMTP settings:"
	ask smtp_host "SMTP host (e.g. smtp.resend.com; press Enter to skip for now)"
	if [ -z "$smtp_host" ]; then
		# The server won't start without mail settings, so fill in placeholders:
		# the site loads, but no sign-in email can be sent until the real ones
		# are in $DIR/.env.
		smtp_host="mail-not-set-up.invalid"
		smtp_port="587"
		smtp_user="not-set-up"
		smtp_password="not-set-up"
		smtp_from="Airobot <noreply@$domain>"
		echo "Mail skipped. The site will load, but nobody can sign in until you put"
		echo "real SMTP settings in $DIR/.env (see deploy/README.md) and run:"
		echo "  cd $DIR && docker compose up -d"
	else
		ask smtp_port "SMTP port" "587"
		ask smtp_user "SMTP username"
		ask_secret smtp_password "SMTP password (not shown)"
		ask smtp_from "Send as" "Airobot <noreply@$domain>"
		[ -n "$smtp_user" ] && [ -n "$smtp_password" ] ||
			fail "the SMTP username and password can't be empty; run the script again."
	fi
	for value in "$domain" "$smtp_host" "$smtp_port" "$smtp_user" "$smtp_password" "$smtp_from"; do
		case "$value" in
			*\'* | *$'\n'*) fail "settings can't contain a ' or a line break." ;;
		esac
	done
	vapid=$(openssl rand -base64 32 | tr '+/' '-_' | tr -d '=\n')
	umask 077
	cat >.env <<EOF
# Made by install.sh on $(date -u +%F). Private: keep it, and a copy of
# PROVIDER_KEY_SECRET somewhere safe; it can never change once used.
DOMAIN='$domain'
AIROBOT_VERSION='latest'
POSTGRES_PASSWORD='$(openssl rand -hex 24)'
PROVIDER_KEY_SECRET='$(openssl rand -hex 32)'
VAPID_PRIVATE_KEY='$vapid'
VAPID_SUBJECT='mailto:noreply@$domain'
SMTP_HOST='$smtp_host'
SMTP_PORT='$smtp_port'
SMTP_USERNAME='$smtp_user'
SMTP_PASSWORD='$smtp_password'
SMTP_FROM='$smtp_from'
RUST_LOG='info'
EOF
	chmod 600 .env
	echo "Saved to $DIR/.env (readable by root only). Secrets were made here and shown nowhere."
fi
domain=$(sed -n "s/^DOMAIN='\{0,1\}\([^']*\)'\{0,1\}$/\1/p" .env | head -1)

# The certificate needs the domain to point here first.
public_ip=$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{for (i = 1; i < NF; i++) if ($i == "src") print $(i + 1)}')
domain_ip=$(getent ahostsv4 "$domain" 2>/dev/null | awk 'NR==1 {print $1}')
if [ -n "$public_ip" ] && [ "$domain_ip" != "$public_ip" ]; then
	echo
	echo "Warning: $domain points at ${domain_ip:-nothing yet}, but this server is $public_ip."
	echo "Set its A record to $public_ip (and www too). HTTPS starts working once it does."
fi

say "6/6  Starting"
if ! docker compose pull; then
	fail "the Airobot image couldn't be downloaded. Either no release is published yet, or the package is private (see Releases in $DIR/README.md)."
fi
docker run --rm -e DOMAIN="$domain" -v "$DIR/Caddyfile:/etc/caddy/Caddyfile:ro" caddy:2 \
	caddy validate --config /etc/caddy/Caddyfile >/dev/null
# An install from before Caddy's address was reserved has the old network;
# rebuild it (only the containers and network go: the data stays).
if docker network inspect airobot_airobot >/dev/null 2>&1; then
	ip_range=$(docker network inspect airobot_airobot --format '{{range .IPAM.Config}}{{.IPRange}}{{end}}')
	if [ "$ip_range" != "172.30.0.128/25" ]; then
		docker compose down
	fi
fi
docker compose up -d

echo "Waiting for Airobot to answer..."
for _ in $(seq 1 60); do
	if docker compose exec -T caddy wget -q -O /dev/null http://app:8080/health 2>/dev/null; then
		ready=1
		break
	fi
	sleep 3
done
if [ -z "${ready:-}" ]; then
	docker compose logs --tail 20 app
	fail "Airobot didn't start; the lines above say why. Fix $DIR/.env and run this script again."
fi

say "Airobot is running."
cat <<EOF

  Open https://$domain and sign in first: the first account becomes the admin.
  (HTTPS works once the domain points here; Caddy keeps trying until then.)

  Next, for your safety:
    - Change the root password:  passwd
    - Log in with an SSH key, then turn off password login
      (PasswordAuthentication no in /etc/ssh/sshd_config, then systemctl restart ssh).
    - Nightly backups and updates: see $DIR/README.md.

  Logs: cd $DIR && docker compose logs -f app
EOF
