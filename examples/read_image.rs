//! Prints basic metadata and the first sector from an EWF image.

use std::env;
use std::io::{Error, ErrorKind};

fn main() -> ewf_image::Result<()> {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| "read_image".to_string());
    let Some(image_path) = args.next() else {
        return Err(
            Error::new(ErrorKind::InvalidInput, format!("usage: {program} <image>")).into(),
        );
    };

    let image = ewf_image::Image::open(&image_path)?;
    let info = image.info();

    println!("file: {}", image.filename().display());
    println!("format: {:?}", info.format);
    println!("profile: {:?}", info.format_profile);
    println!("logical size: {}", info.logical_size);
    println!("chunk size: {}", info.chunk_size);
    println!("segments: {}", info.segment_count);
    println!("acquisition complete: {}", info.acquisition_complete);

    if let Some(case_number) = image.header_value("case_number") {
        println!("case number: {case_number}");
    }
    if let Some(evidence_number) = image.header_value("evidence_number") {
        println!("evidence number: {evidence_number}");
    }
    if let Some(examiner) = image.header_value("examiner_name") {
        println!("examiner: {examiner}");
    }

    for index in 0..image.number_of_hash_values() {
        if let Some(identifier) = image.hash_value_identifier(index)
            && let Some(value) = image.hash_value(identifier)
        {
            println!("hash {identifier}: {value}");
        }
    }

    #[cfg(feature = "verify")]
    {
        let verification = image.verify()?;
        println!("MD5 match: {:?}", verification.md5_match);
        println!("SHA1 match: {:?}", verification.sha1_match);
    }

    Ok(())
}
