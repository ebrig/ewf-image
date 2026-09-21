//! Acquire a regular file to bounded sequential EWF2 output.
use ewf_image::{SequentialOptions, SequentialWriter};
use std::fs::File;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let source = args
        .next()
        .ok_or("usage: sequential SOURCE OUTPUT [zlib]")?;
    let output = args.next().ok_or("missing output")?;
    let mut source = File::open(source)?;
    let mut options = SequentialOptions::new(source.metadata()?.len());
    // 32 MiB raw capacity per staged segment, for a small scratch budget.
    options.chunks_per_segment = 1024;
    if args.next().as_deref() == Some("zlib") {
        options.write.compression = ewf_image::WriteCompression::Zlib;
    }
    let mut writer = SequentialWriter::create(output, options)?;
    std::io::copy(&mut source, &mut writer)?;
    let result = writer.finish()?;
    let verified = ewf_image::Image::open(&result.segment_paths[0])?.verify_with_options(
        &ewf_image::VerifyOptions::default().with_expected_sha256(result.computed_sha256),
    )?;
    if verified.references_match() != Some(true) {
        return Err("verification mismatch".into());
    }
    println!(
        "{} bytes; {} segments; verified SHA256",
        result.logical_size,
        result.segment_paths.len()
    );
    Ok(())
}
