//! Links the WebAssembly module with a 2 MiB stack instead of rust-lld's
//! default 1 MiB.
//!
//! sqlparser's own recursion limit (50 levels) doesn't bound its stack use in
//! bytes: 10,000 nested `f(` reach about 512 KiB before the limit stops them,
//! which leaves less than 2x headroom on 1 MiB. A stack overflow in the module
//! is a trap (src/lib/wasm/index.ts recovers, but the call fails).
//!
//! Only the cdylib for wasm32 gets the flag, so host builds, tests and the
//! other crates' caches are untouched. scripts/build-wasm.mjs checks the
//! result: `__stack_pointer` starts at this size (the stack comes first in
//! memory on wasm32-unknown-unknown).

const STACK_SIZE: u32 = 2 * 1024 * 1024;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo::rustc-link-arg-cdylib=-zstack-size={STACK_SIZE}");
    }
}
