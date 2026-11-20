// `sqlx::migrate!` embeds the migrations at compile time, but cargo does not watch the folder: a
// new migration file would otherwise be left out until something else in this crate changed.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
