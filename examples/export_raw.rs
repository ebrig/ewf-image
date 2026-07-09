//! Exports the logical media stream from an EWF image to a raw file.

use std::env;
use std::fs::File;
use std::io::{self, Error, ErrorKind};

fn main() -> ewf_image::Result<()> {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| "export_raw".to_string());
    let (Some(image_path), Some(raw_path)) = (args.next(), args.next()) else {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("usage: {program} <image> <output.raw>"),
        )
        .into());
    };

    let image = ewf_image::Image::open(&image_path)?;
    let mut cursor = image.cursor();
    let mut output = File::create(&raw_path)?;
    let bytes = io::copy(&mut cursor, &mut output)?;

    println!("wrote {bytes} bytes to {raw_path}");
    Ok(())
}
