//! Prints the logical single-file catalog from an EWF image.

use std::env;
use std::io::{Error, ErrorKind};

fn main() -> ewf_image::Result<()> {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| "inspect_logical".to_string());
    let Some(image_path) = args.next() else {
        return Err(
            Error::new(ErrorKind::InvalidInput, format!("usage: {program} <image>")).into(),
        );
    };

    let image = ewf_image::Image::open(&image_path)?;
    let Some(root) = image.root_file_entry() else {
        println!("no logical single-file catalog");
        return Ok(());
    };

    print_entry(root, 0);
    Ok(())
}

fn print_entry(entry: &ewf_image::SingleFileEntry, depth: usize) {
    let indent = "  ".repeat(depth);
    let name = entry.name().unwrap_or("<root>");
    let size = entry.size().unwrap_or(0);
    let entry_type = entry.entry_type();

    println!("{indent}{name} {entry_type:?} {size} bytes");

    for child in &entry.children {
        print_entry(child, depth + 1);
    }
}
