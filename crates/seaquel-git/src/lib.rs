//! Seaquel's git operations for shared projects: clone, init, pull with
//! conflicts, push, status with ahead/behind counts, commit, conflict
//! resolution and remotes, over libgit2. libgit2 blocks, so every call runs on
//! a blocking thread. Core exposes it behind its `git` feature.
