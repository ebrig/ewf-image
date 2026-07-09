//! Writes an input file into a new EWF image.

use std::env;
use std::fs::File;
use std::io::{self, Error, ErrorKind};

fn main() -> ewf_image::Result<()> {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| "write_image".to_string());
    let (Some(raw_path), Some(image_path)) = (args.next(), args.next()) else {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("usage: {program} <input.raw> <output.Ex01>"),
        )
        .into());
    };

    let mut input = File::open(&raw_path)?;

    let mut options = ewf_image::WriteOptions {
        format: ewf_image::WriteFormat::Ewf2Physical,
        compression: ewf_image::WriteCompression::Zlib,
        ..ewf_image::WriteOptions::default()
    };
    options.metadata.set_header_value("case_number", "CASE-001");
    options
        .metadata
        .set_header_value("acquiry_software", env!("CARGO_PKG_NAME"));
    options
        .metadata
        .set_header_value("acquiry_software_version", env!("CARGO_PKG_VERSION"));

    let mut writer = ewf_image::EwfWriter::create(&image_path, options)?;
    let bytes = io::copy(&mut input, &mut writer)?;
    let result = writer.finish()?;

    println!("read {bytes} raw bytes");
    println!("wrote {} segment(s)", result.segment_paths.len());
    for path in result.segment_paths {
        println!("{}", path.display());
    }

    Ok(())
}
