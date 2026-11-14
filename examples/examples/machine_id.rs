//! Print the application host's hashed Kitty drag-and-drop machine ID.
//! Run with `cargo run -p examples --example machine_id`.

use uncurses_dragdrop::MachineId;

fn main() -> std::io::Result<()> {
    println!("{}", MachineId::current()?);
    Ok(())
}
