use crate::{Result, invalid};
use std::{fs::File, io::Read, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Input {
    Ewf,
    Aff4,
    Raw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Output {
    E01,
    Ex01,
    Lx01,
    Aff4,
    Raw,
}

impl Output {
    pub(crate) fn from_path(path: &Path) -> Result<Self> {
        match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
            "E01" => Ok(Self::E01),
            "Ex01" => Ok(Self::Ex01),
            "Lx01" => Ok(Self::Lx01),
            s if s.eq_ignore_ascii_case("aff4") => Ok(Self::Aff4),
            "raw" | "dd" | "img" | "bin" => Ok(Self::Raw),
            _ => Err(invalid(
                "use an output filename ending in .E01, .Ex01, .Lx01, .aff4, or .raw",
            )),
        }
    }
}

pub(crate) fn detect(path: &Path) -> Result<Input> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(invalid(
            "use acquire for a physical device; image operations require regular files",
        ));
    }
    if ewf_image::check_file_signature(path)? {
        return Ok(Input::Ewf);
    }
    let mut header = [0; 4];
    let count = File::open(path)?.read(&mut header)?;
    if count == 4 && matches!(&header, b"PK\x03\x04" | b"PK\x05\x06") {
        return Ok(Input::Aff4);
    }
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "raw" | "dd" | "img" | "bin") {
        return Ok(Input::Raw);
    }
    Err(invalid(
        "unrecognized image signature; raw images must use .raw, .dd, .img, or .bin",
    ))
}

pub(crate) fn parse_hash(text: &str) -> Result<[u8; 32]> {
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid(
            "SHA256 must contain exactly 64 hexadecimal characters",
        ));
    }
    let mut hash = [0; 32];
    for (index, value) in hash.iter_mut().enumerate() {
        *value = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)?;
    }
    Ok(hash)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
