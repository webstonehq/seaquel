# SSH test fixtures

These keys are for tests only. They were generated for this repo and
authorize nothing but the local `ssh` test container in
`../docker-compose.yml` (and the same container in CI). Don't reuse them
anywhere else.

- `id_ed25519` / `.pub`: no passphrase.
- `id_ed25519_passphrase` / `.pub`: passphrase `seaquel-test-passphrase`.
- `init/10-seaquel-ssh.sh`: the container's custom init. It installs both
  public keys for the user `seaquel` (whose password is
  `seaquel-test-password`), turns on TCP forwarding and relaxes OpenSSH's
  per-source limits for the parallel tests. It must stay executable (mode
  755), or the image skips it.

The live tests in `crates/seaquel-ssh/tests/tunnel.rs` and
`crates/seaquel-core/tests/ssh.rs` use them when `SEAQUEL_TEST_SSH` is set.
