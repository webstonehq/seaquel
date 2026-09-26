//! Seaquel's SSH tunnels. It opens a local port that forwards through an SSH
//! server to a database host, authenticating with a password or a key file,
//! checks the server's host key against known_hosts with trust on first use,
//! and ends every forward when a tunnel is closed. Core owns the running
//! tunnels behind its `ssh` feature.
