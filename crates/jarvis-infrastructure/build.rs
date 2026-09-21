// SQLx embeds migrations at compile time from `migrations/sqlite`. Its hash
// inputs are the migration file bytes, so the build must re-run when those files
// change; otherwise a stale copy is silently compiled in.
fn main() {
    println!("cargo:rerun-if-changed=../../migrations/sqlite");
}
