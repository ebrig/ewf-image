use std::{
    fs::File,
    io::{self, BufRead, Read},
    path::Path,
};

use ewf_image::{EwfPassword, Image};

use crate::{Result, invalid};

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
    Ok(match password {
        Some(password) => Image::open_with_password(path, password)?,
        None => Image::open(path)?,
    })
}
