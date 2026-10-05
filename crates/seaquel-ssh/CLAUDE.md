# seaquel-ssh

Moved from the root `CLAUDE.md`, which has the overview and the crate map. "Above" and "below" may point at sections that now live in another `CLAUDE.md`.

- `seaquel-ssh` — SSH tunnels over russh 0.48: a local port forwarded through a bastion, password or key-file auth, and the host-key check against known_hosts. An unknown key fails with `UNKNOWN_HOST_KEY` and its `SHA256:…` fingerprint; the retry passes that fingerprint as `trustHostKey`, and only a key with exactly that fingerprint is recorded. `HOST_KEY_MISMATCH` is never accepted. A key of an algorithm known_hosts doesn't hold for the host counts as unknown, not a mismatch (russh compares only keys of the same algorithm). Closing a tunnel (or dropping it) aborts every forward and ends the SSH session. The error codes are the ones the TS host-key prompt matches on; keep them. Live tests need `SEAQUEL_TEST_SSH` (below).

## SSH tests

- SSH tests (`crates/seaquel-ssh/tests/tunnel.rs`, `crates/seaquel-core/tests/ssh.rs`) use the compose file's `ssh` service (OpenSSH on `127.0.0.1:2222`, user `seaquel`, the fixture keys in `e2e/test-databases/ssh/`) and `SEAQUEL_TEST_SSH='{"host":"127.0.0.1","port":2222,"remote_host":"postgres","remote_port":5432}'`. The image keeps its config in an anonymous volume, so recreate it with `-V` after changing `ssh/init`. The compose file's `blackhole` service (`alpine/socat`, `TCP-LISTEN:5432` running `sleep 3600`, no host port) accepts and never answers: Core's `a_dropped_connect_closes_its_tunnel` (`tests/connect.rs`) reaches it through the bastion as `blackhole:5432`, so start it with `docker compose … up -d blackhole` beside `ssh`; CI's engines job runs it on the services' network and runs `--test connect` with `--test ssh`.
