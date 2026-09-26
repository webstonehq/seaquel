#!/usr/bin/with-contenv bash
# shellcheck shell=bash
#
# Custom init for the SSH test container. linuxserver/openssh-server runs
# every executable file in /custom-cont-init.d after its own config step and
# before sshd starts. Safe to run again on a restart.
#
# The image runs sshd as its one user (USER_NAME, `seaquel`), so no other
# account can log in. That user takes both its password and the fixture keys
# in /seaquel-keys: one without a passphrase and one with.
#
# It also turns on local forwarding, which the image turns off.

set -euo pipefail

CONFIG=/config/sshd/sshd_config
AUTHORIZED=/config/.ssh/authorized_keys

cat /seaquel-keys/id_ed25519.pub /seaquel-keys/id_ed25519_passphrase.pub >"$AUTHORIZED"
chmod 600 "$AUTHORIZED"
chown "${PUID:-1000}:${PGID:-1000}" "$AUTHORIZED"

# sshd keeps the first value it reads, so change the image's line in place.
sed -i 's/^AllowTcpForwarding .*/AllowTcpForwarding yes/' "$CONFIG"

# The tests connect many times at once from one address, some with a wrong
# password or dropping the connection at the host-key check. OpenSSH's
# per-source penalties would then refuse that address for a while, and
# MaxStartups would drop connections that haven't logged in yet.
sed -i '/^PerSourcePenalties /d; /^MaxStartups /d' "$CONFIG"
sed -i '1i PerSourcePenalties no\nMaxStartups 200:30:400' "$CONFIG"

echo "[seaquel-ssh] password and key logins for ${USER_NAME}; forwarding on"
