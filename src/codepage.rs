use encoding_rs::{
    Encoding, GBK, SHIFT_JIS, WINDOWS_874, WINDOWS_1250, WINDOWS_1251, WINDOWS_1252, WINDOWS_1253,
    WINDOWS_1254, WINDOWS_1255, WINDOWS_1256, WINDOWS_1257, WINDOWS_1258,
};

use crate::types::HeaderCodepage;

pub(crate) fn decode_header_bytes(bytes: &[u8], codepage: HeaderCodepage) -> String {
    if let Some(encoding) = header_encoding(codepage) {
        let (text, _, _) = encoding.decode(bytes);
        text.into_owned()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

pub(crate) fn encode_header_text(text: &str, codepage: HeaderCodepage) -> Vec<u8> {
    if let Some(encoding) = header_encoding(codepage) {
        let (bytes, _, _) = encoding.encode(text);
        bytes.into_owned()
    } else {
        text.as_bytes().to_vec()
    }
}

fn header_encoding(codepage: HeaderCodepage) -> Option<&'static Encoding> {
    Some(match codepage {
        HeaderCodepage::Ascii => return None,
        HeaderCodepage::Windows874 => WINDOWS_874,
        HeaderCodepage::Windows932 => SHIFT_JIS,
        HeaderCodepage::Windows936 => GBK,
        HeaderCodepage::Windows1250 => WINDOWS_1250,
        HeaderCodepage::Windows1251 => WINDOWS_1251,
        HeaderCodepage::Windows1252 => WINDOWS_1252,
        HeaderCodepage::Windows1253 => WINDOWS_1253,
        HeaderCodepage::Windows1254 => WINDOWS_1254,
        HeaderCodepage::Windows1255 => WINDOWS_1255,
        HeaderCodepage::Windows1256 => WINDOWS_1256,
        HeaderCodepage::Windows1257 => WINDOWS_1257,
        HeaderCodepage::Windows1258 => WINDOWS_1258,
    })
}
