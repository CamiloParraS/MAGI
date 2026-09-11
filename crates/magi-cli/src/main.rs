//! magi-cli: dev/test CLI (doctor, roots, index, daemon, search, eval — added
//! milestone by milestone; see SPEC.md §7).

fn main() -> anyhow::Result<()> {
    println!("magi-core says: {}", magi_core::ping());
    Ok(())
}
