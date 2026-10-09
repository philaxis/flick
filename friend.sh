#!/bin/bash
# Lends this Mac for a build session. Run it in Terminal, inside a throwaway
# STANDARD (non-admin) account. Needs no password and changes nothing outside
# this account's home folder: everything lives in ~/borrow.
# Closing this window (or Ctrl+C) disconnects at once. Deleting the account removes it all.
set -euo pipefail

GO_VERSION=go1.27.2
GO_SHA_arm64=76812b213b1b2302c978d28fa52fa92d541704b9e7d9d5db8002c50e4018c4c5
GO_SHA_amd64=587b59182488b23aa6e5fc25110405a3e0e5b38ed2f5b2f46ed13c32aee356fe
TS_VERSION=v1.104.1
PORT=${BORROW_PORT:-2222}
B="$HOME/borrow"

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
die() { printf '\n\033[31m%s\033[0m\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Darwin ] || die "This is for macOS."
: "${BORROW_PUB:?BORROW_PUB is missing}"
if id -Gn | tr ' ' '\n' | grep -qx admin && [ "${BORROW_ALLOW_ADMIN:-}" != 1 ]; then
  die "This account is an administrator. Please run this inside the new standard account instead, so nothing of yours can be reached."
fi
case "$(uname -m)" in arm64) A=arm64 ;; x86_64) A=amd64 ;; *) die "Unknown CPU" ;; esac

umask 077
mkdir -p "$B"/{dl,bin,ts,ssh,log}
echo $$ > "$B/pid"

cleanup() {
  trap - EXIT INT TERM HUP
  "$B/bin/tailscale" --socket="$B/ts/sock" logout >/dev/null 2>&1 || true
  for f in "$B/ssh/pid" "$B/ts/pid" "$B/caffeinate.pid"; do
    [ -f "$f" ] && kill "$(cat "$f")" 2>/dev/null || true
  done
  printf '\n\033[1mDisconnected. Nobody can reach this Mac any more.\033[0m\n'
}
trap cleanup EXIT INT TERM HUP

if [ -n "${BORROW_KEY:-}${BORROW_TEST_BUILD:-}" ] && [ ! -x "$B/bin/tailscaled" ]; then
  say "1/3  Fetching the private network tool (about 2 minutes)…"
  sha_var=GO_SHA_$A
  curl -fsSL -o "$B/dl/go.tgz" "https://go.dev/dl/$GO_VERSION.darwin-$A.tar.gz"
  echo "${!sha_var}  $B/dl/go.tgz" | shasum -a 256 -c - >/dev/null || die "Download did not match its checksum."
  tar -C "$B" -xzf "$B/dl/go.tgz" && rm "$B/dl/go.tgz"
  # Built from Tailscale's published source at a pinned version (checked by Go's checksum database).
  GOPATH="$B/gopath" GOCACHE="$B/gocache" GOBIN="$B/bin" CGO_ENABLED=0 GOFLAGS=-modcacherw \
    "$B/go/bin/go" install "tailscale.com/cmd/tailscale@$TS_VERSION" "tailscale.com/cmd/tailscaled@$TS_VERSION" \
    > "$B/log/go.log" 2>&1 || die "Could not build the network tool. See $B/log/go.log"
  rm -rf "$B/gocache" "$B/gopath" "$B/go"
fi

say "2/3  Opening a door for one key only…"
[ -f "$B/ssh/host" ] || ssh-keygen -q -t ed25519 -N '' -f "$B/ssh/host"
printf '%s\n' "$BORROW_PUB" > "$B/ssh/authorized_keys"
cat > "$B/ssh/config" <<CONF
Port $PORT
ListenAddress 127.0.0.1
HostKey $B/ssh/host
PidFile $B/ssh/pid
AuthorizedKeysFile $B/ssh/authorized_keys
AllowUsers $(id -un)
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
X11Forwarding no
Subsystem sftp /usr/libexec/sftp-server
CONF
/usr/sbin/sshd -f "$B/ssh/config" -E "$B/log/sshd.log"
caffeinate -i -w $$ & echo $! > "$B/caffeinate.pid"

if [ -n "${BORROW_KEY:-}" ]; then
  say "3/3  Connecting…"
  "$B/bin/tailscaled" --tun=userspace-networking --socket="$B/ts/sock" --statedir="$B/ts/state" \
    --port=0 > "$B/log/tailscaled.log" 2>&1 & echo $! > "$B/ts/pid"
  printf '%s' "$BORROW_KEY" > "$B/ts/key"
  for _ in $(seq 30); do [ -S "$B/ts/sock" ] && break; sleep 1; done
  "$B/bin/tailscale" --socket="$B/ts/sock" up --auth-key="file:$B/ts/key" \
    --hostname="${BORROW_NAME:-borrowed-mac}" --accept-dns=false --accept-routes=false --timeout=90s \
    || { rm -f "$B/ts/key"; die "Could not connect. The invitation may have expired."; }
  rm -f "$B/ts/key"
  IP=$("$B/bin/tailscale" --socket="$B/ts/sock" ip -4)
else
  IP="127.0.0.1 (local rehearsal, no network)"
fi

cat <<DONE

  ✅  Connected.   $IP
      Door fingerprint: $(ssh-keygen -lf "$B/ssh/host.pub" | awk '{print $2}')

  Keep this window open.  To stop at any moment: close this window.
  When we are done, delete this account (System Settings → Users & Groups).

DONE
while kill -0 "$(cat "$B/ssh/pid" 2>/dev/null)" 2>/dev/null && [ -f "$B/pid" ]; do sleep 2; done
