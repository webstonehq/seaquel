//! `SEAQUEL_DATA_DIR` read from the real environment. It is its own test
//! binary with one test, because setting a variable races with any other
//! thread that reads the environment.

#![cfg(not(target_arch = "wasm32"))]

use seaquel_storage::{data_dir, DATA_DIR_ENV};

#[test]
fn seaquel_data_dir_overrides_the_platform_dir() {
    let dir = std::env::temp_dir().join("seaquel-data-dir-env-test");
    std::env::set_var(DATA_DIR_ENV, &dir);
    assert_eq!(data_dir("app.seaquel.desktop").unwrap(), dir);
    assert_eq!(data_dir("app.seaquel.desktop.dev").unwrap(), dir);
    assert!(!dir.exists(), "data_dir creates nothing");

    std::env::set_var(DATA_DIR_ENV, "");
    let fallback = data_dir("app.seaquel.desktop").unwrap();
    assert_eq!(
        fallback,
        dirs::data_dir().unwrap().join("app.seaquel.desktop")
    );
}
