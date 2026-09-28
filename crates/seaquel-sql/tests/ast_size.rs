//! `column_refs` on big input (phase 5b probe, I1). Core computes it for every
//! SELECT a run or page carries, on text up to the web's 8 MiB frame, and
//! sqlparser tokenizes and parses the whole input first: about 1.3 KB of heap
//! and 0.2 µs per byte for a dense select list (`SELECT 1,1,1,…`), so an
//! 8 MiB statement took gigabytes. Past [`MAX_COLUMN_REFS_BYTES`] it answers
//! `None` without parsing.
//!
//! Its own test binary, since the counting allocator is global. Native
//! only, so `Instant` is allowed here.

#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use seaquel_sql::ast::{column_refs, MAX_COLUMN_REFS_BYTES};
use seaquel_sql::SqlEngine;

struct Counting;
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to `System`, only counting sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let now = CURRENT.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
        PEAK.fetch_max(now, Ordering::SeqCst);
        // SAFETY: the caller's contract, passed on.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::SeqCst);
        // SAFETY: the caller's contract, passed on.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Peak heap growth (bytes) and time of `f`.
fn measure<T>(f: impl FnOnce() -> T) -> (T, usize, Duration) {
    let base = CURRENT.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let start = Instant::now();
    let out = f();
    let elapsed = start.elapsed();
    (
        out,
        PEAK.load(Ordering::SeqCst).saturating_sub(base),
        elapsed,
    )
}

fn select_list(bytes: usize) -> String {
    format!("SELECT {}t.a FROM t", "t.a,".repeat(bytes / 4))
}

#[test]
fn an_8_mib_select_is_refused_without_parsing() {
    let sql = format!("SELECT {}1 FROM t", "1,".repeat(4 * 1024 * 1024));
    for engine in [
        SqlEngine::Postgres,
        SqlEngine::Mysql,
        SqlEngine::Mssql,
        SqlEngine::Duckdb,
    ] {
        let (refs, peak, elapsed) = measure(|| column_refs(&sql, engine));
        assert_eq!(refs, None);
        assert!(peak < 64 * 1024, "{engine:?}: {peak} bytes allocated");
        assert!(
            elapsed < Duration::from_millis(50),
            "{engine:?}: {elapsed:?}"
        );
    }
}

#[test]
fn just_past_the_limit_is_none_and_just_under_still_maps() {
    let under = select_list(MAX_COLUMN_REFS_BYTES - 64);
    assert!(under.len() <= MAX_COLUMN_REFS_BYTES);
    let refs = column_refs(&under, SqlEngine::Postgres).expect("maps under the limit");
    assert_eq!(refs.len(), (MAX_COLUMN_REFS_BYTES - 64) / 4 + 1);

    let over = select_list(MAX_COLUMN_REFS_BYTES + 64);
    assert!(over.len() > MAX_COLUMN_REFS_BYTES);
    assert_eq!(column_refs(&over, SqlEngine::Postgres), None);
}

#[test]
fn the_worst_case_under_the_limit_is_bounded() {
    // The densest select list measured: one item per two bytes.
    let body = "1,".repeat((MAX_COLUMN_REFS_BYTES - 32) / 2);
    let sql = format!("SELECT {body}1 FROM t");
    assert!(sql.len() <= MAX_COLUMN_REFS_BYTES);
    let (refs, peak, _) = measure(|| column_refs(&sql, SqlEngine::Postgres));
    assert!(refs.is_some());
    // ~86 MB measured in a release build (sqlparser's `Expr` is large).
    assert!(peak < 160 * 1024 * 1024, "{peak} bytes allocated");
}
