use std::{
    fs::File,
    io::{self, BufRead, Read},
    path::Path,
};

use ewf_image::{EwfPassword, Image, OpenOptions};

use crate::{Result, invalid};

const MAXIMUM_OPEN_EWF_HANDLES: usize = 32;

fn open_options() -> OpenOptions {
    // Keep room for source and destination files, publication journals, and
    // other process resources even when an image contains many segments.
    OpenOptions::default().with_maximum_open_handles(Some(MAXIMUM_OPEN_EWF_HANDLES))
}

pub(crate) fn read(path: &Path) -> Result<EwfPassword> {
    let mut bytes = Vec::new();
    if path == Path::new("-") {
        io::stdin().lock().take(34).read_until(b'\n', &mut bytes)?;
    } else {
        File::open(path)?.take(34).read_to_end(&mut bytes)?;
    }
    if bytes.ends_with(b"\r\n") {
        bytes.truncate(bytes.len() - 2);
    } else if bytes.ends_with(b"\n") {
        bytes.truncate(bytes.len() - 1);
    }
    if bytes.is_empty() || bytes.len() > 32 {
        bytes.fill(0);
        return Err(invalid("EWF password must contain 1 to 32 bytes"));
    }
    Ok(EwfPassword::from_bytes(bytes))
}

pub(crate) fn open(path: &Path, password: Option<&EwfPassword>) -> Result<Image> {
    let options = open_options();
    Ok(match password {
        Some(password) => Image::open_with_options_and_password(path, options, password)?,
        None => Image::open_with_options(path, options)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_reader_reserves_file_descriptors() {
        assert_eq!(
            open_options().maximum_open_handles(),
            Some(MAXIMUM_OPEN_EWF_HANDLES)
        );
    }
}
