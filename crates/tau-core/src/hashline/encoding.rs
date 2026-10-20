//! File-encoding sniffing and round-trip (BOM + UTF-16, #207 §6): the
//! pipeline works on UTF-8 text, but a file on disk may carry a byte-order
//! mark (which would otherwise pollute the first line's canon) or be UTF-16
//! (which `from_utf8` rejects outright). `sniff` decodes the bytes to UTF-8
//! with the BOM stripped and records the encoding; `encode` re-encodes to
//! that same form, so an `edit` round-trips a BOM'd or UTF-16 file byte-for-
//! byte outside the edited span.

/// The file's on-disk encoding, as sniffed from its leading bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    #[default]
    Utf8,
    Utf16Le,
    Utf16Be,
}

/// What `sniff` found: the encoding and whether a BOM was present (and
/// stripped from the decoded text).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sniffed {
    pub encoding: Encoding,
    pub bom: bool,
}

/// Decode `bytes` to UTF-8, detecting a BOM (UTF-8 / UTF-16LE / UTF-16BE)
/// and stripping it, and recording the encoding. A file with no BOM is UTF-8;
/// the decoded text is rejected (not guessed) if it is not valid UTF-8 —
/// a BOM-less non-UTF-8 file is out of scope (the loud refusal stands).
pub fn sniff(bytes: &[u8]) -> Result<(String, Sniffed), String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        let text = std::str::from_utf8(rest)
            .map_err(|e| format!("not valid UTF-8 (after BOM): {e}"))?
            .to_owned();
        return Ok((
            text,
            Sniffed {
                encoding: Encoding::Utf8,
                bom: true,
            },
        ));
    }
    // UTF-16 BOMs: FF FE (LE) / FE FF (BE).
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let text =
            utf16_to_utf8(&bytes[2..], true).map_err(|e| format!("invalid UTF-16LE: {e}"))?;
        return Ok((
            text,
            Sniffed {
                encoding: Encoding::Utf16Le,
                bom: true,
            },
        ));
    }
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let text =
            utf16_to_utf8(&bytes[2..], false).map_err(|e| format!("invalid UTF-16BE: {e}"))?;
        return Ok((
            text,
            Sniffed {
                encoding: Encoding::Utf16Be,
                bom: true,
            },
        ));
    }
    // No BOM: UTF-8 (the pipeline's native form).
    let text = std::str::from_utf8(bytes)
        .map_err(|e| format!("not valid UTF-8: {e}"))?
        .to_owned();
    Ok((text, Sniffed::default()))
}

/// Encode `text` to the file's on-disk form: the sniffed encoding, with the
/// BOM re-added when it was present. This is what makes an `edit` preserve a
/// BOM'd or UTF-16 file.
#[must_use]
pub fn encode(text: &str, sniffed: &Sniffed) -> Vec<u8> {
    match sniffed.encoding {
        Encoding::Utf8 => {
            if sniffed.bom {
                let mut out = vec![0xEF, 0xBB, 0xBF];
                out.extend_from_slice(text.as_bytes());
                out
            } else {
                text.as_bytes().to_vec()
            }
        }
        Encoding::Utf16Le => {
            let mut out = if sniffed.bom {
                vec![0xFF, 0xFE]
            } else {
                Vec::new()
            };
            out.extend(utf8_to_utf16(text, true));
            out
        }
        Encoding::Utf16Be => {
            let mut out = if sniffed.bom {
                vec![0xFE, 0xFF]
            } else {
                Vec::new()
            };
            out.extend(utf8_to_utf16(text, false));
            out
        }
    }
}

/// Decode UTF-16 (LE or BE) code units to a UTF-8 string, handling surrogate
/// pairs. A lone surrogate or a trailing odd byte is an error (not a guess).
fn utf16_to_utf8(bytes: &[u8], le: bool) -> Result<String, String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("odd byte length".into());
    }
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let lo = u32::from(bytes[i]);
        let hi = u32::from(bytes[i + 1]);
        let unit = if le { lo | hi << 8 } else { hi | lo << 8 };
        i += 2;
        if (0xD800..0xDC00).contains(&unit) {
            // High surrogate: a low surrogate must follow.
            if i + 2 > bytes.len() {
                return Err("unterminated high surrogate".into());
            }
            let lo2 = u32::from(bytes[i]);
            let hi2 = u32::from(bytes[i + 1]);
            let low = if le { lo2 | hi2 << 8 } else { hi2 | lo2 << 8 };
            if !(0xDC00..0xE000).contains(&low) {
                return Err("high surrogate not followed by low".into());
            }
            i += 2;
            let cp = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
            out.push(char::from_u32(cp).ok_or("invalid code point")?);
        } else if (0xDC00..0xE000).contains(&unit) {
            return Err("unexpected low surrogate".into());
        } else {
            out.push(char::from_u32(unit).ok_or("invalid code point")?);
        }
    }
    Ok(out)
}

/// Encode a UTF-8 string to UTF-16 (LE or BE) code units, astral planes as
/// surrogate pairs.
fn utf8_to_utf16(text: &str, le: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for c in text.chars() {
        // char has no `From` into u32; the cast is a widening value view.
        #[allow(clippy::as_conversions)]
        let cp = c as u32;
        let units: [u32; 2] = if cp > 0xFFFF {
            let c = cp - 0x10000;
            [0xD800 + (c >> 10), 0xDC00 + (c & 0x3FF)]
        } else {
            [cp, 0]
        };
        let count = usize::from(cp > 0xFFFF) + 1;
        for u in &units[..count] {
            if le {
                out.push(u8::try_from(u & 0xFF).expect("masked to 8 bits"));
                out.push(u8::try_from((u >> 8) & 0xFF).expect("masked to 8 bits"));
            } else {
                out.push(u8::try_from((u >> 8) & 0xFF).expect("masked to 8 bits"));
                out.push(u8::try_from(u & 0xFF).expect("masked to 8 bits"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_bom_is_utf8() {
        let (text, s) = sniff(b"hello\nworld").unwrap();
        assert_eq!(text, "hello\nworld");
        assert_eq!(s, Sniffed::default());
    }

    #[test]
    fn utf8_bom_is_stripped_and_recorded() {
        let (text, s) = sniff(&[0xEF, 0xBB, 0xBF, b'h', b'e', b'l']).unwrap();
        assert_eq!(text, "hel");
        assert!(s.bom);
        assert_eq!(s.encoding, Encoding::Utf8);
    }

    #[test]
    fn utf16le_roundtrips() {
        // "hi" in UTF-16LE with BOM.
        let bytes = vec![0xFF, 0xFE, b'h', 0, b'i', 0];
        let (text, s) = sniff(&bytes).unwrap();
        assert_eq!(text, "hi");
        assert_eq!(s.encoding, Encoding::Utf16Le);
        assert!(s.bom);
        assert_eq!(encode(&text, &s), bytes);
    }

    #[test]
    fn utf16be_roundtrips() {
        let bytes = vec![0xFE, 0xFF, 0, b'h', 0, b'i'];
        let (text, s) = sniff(&bytes).unwrap();
        assert_eq!(text, "hi");
        assert_eq!(s.encoding, Encoding::Utf16Be);
        assert_eq!(encode(&text, &s), bytes);
    }

    #[test]
    fn astral_plane_surrogate_pair_roundtrips() {
        // U+1F600 (grinning face) is a surrogate pair in UTF-16.
        let s = Sniffed {
            encoding: Encoding::Utf16Le,
            bom: true,
        };
        let bytes = encode("a\u{1F600}b", &s);
        let (text, s2) = sniff(&bytes).unwrap();
        assert_eq!(text, "a\u{1F600}b");
        assert_eq!(s2, s);
    }

    #[test]
    fn lone_surrogate_is_an_error() {
        // A high surrogate with no following low surrogate.
        let bytes = vec![0xFF, 0xFE, 0x34, 0xD8];
        assert!(sniff(&bytes).is_err());
    }

    #[test]
    fn non_utf8_without_bom_is_refused() {
        // 0xFF is not valid UTF-8 leading byte and no BOM matches.
        assert!(sniff(&[0xFF, 0x00, 0x00]).is_err());
    }
}
