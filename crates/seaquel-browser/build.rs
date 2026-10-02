//! Links the WebAssembly module with a 2 MiB stack instead of rust-lld's
//! default 1 MiB, as `crates/seaquel-wasm/build.rs` does for the editor
//! module: Core's library and state calls build deep futures, and the module
//! parses SQL with sqlparser like the editor module. A stack overflow is a
//! trap, which the page's transport recovers from, but the call fails.
//!
//! Only the cdylib for wasm32 gets the flag. scripts/build-wasm.mjs checks
//! the result.

const STACK_SIZE: u32 = 2 * 1024 * 1024;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo::rustc-link-arg-cdylib=-zstack-size={STACK_SIZE}");
    }
}
