// `sqlx::migrate!` embeds `migrations/` at compile time. Without this, adding
// or changing a migration file wouldn't rebuild the crate.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
