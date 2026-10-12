//! What a figure's bytes must look like. The file name says what a figure
//! claims to be; these checks decide whether the bytes agree, so a zip
//! renamed to `.png` is refused. Every check reads only through `get` and
//! checked arithmetic, so no input can make them panic.

const REFUSED: &str = "that file is not a PNG, JPEG, GIF, WebP, SVG, PDF or font";

/// Whether these bytes are a figure, PDF or font this server stores. The
/// error is the sentence shown to whoever uploaded them.
pub fn check_asset(bytes: &[u8]) -> Result<(), String> {
    let accepted = png(bytes)
        || jpeg(bytes)
        || gif(bytes)
        || webp(bytes)
        || svg(bytes)
        || pdf(bytes)
        || sfnt(bytes)
        || woff(bytes);
    if accepted {
        Ok(())
    } else {
        Err(REFUSED.to_string())
    }
}

/// The `len` bytes at `start`, or `None` when they do not all fit.
fn span(bytes: &[u8], start: usize, len: usize) -> Option<&[u8]> {
    let end = start.checked_add(len)?;
    bytes.get(start..end)
}

fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let raw = <[u8; 4]>::try_from(span(bytes, at, 4)?).ok()?;
    Some(u32::from_be_bytes(raw))
}

fn be_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let raw = <[u8; 2]>::try_from(span(bytes, at, 2)?).ok()?;
    Some(u16::from_be_bytes(raw))
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let raw = <[u8; 4]>::try_from(span(bytes, at, 4)?).ok()?;
    Some(u32::from_le_bytes(raw))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn png(bytes: &[u8]) -> bool {
    const SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    if !bytes.starts_with(&SIGNATURE) {
        return false;
    }
    let mut at = SIGNATURE.len();
    loop {
        let Some(length) = be_u32(bytes, at) else {
            return false;
        };
        let Some(kind) = at.checked_add(4).and_then(|start| span(bytes, start, 4)) else {
            return false;
        };
        // Length, type, data and CRC.
        let Some(total) = (length as usize).checked_add(12) else {
            return false;
        };
        let Some(end) = at.checked_add(total) else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        if kind == b"IEND" {
            return end == bytes.len();
        }
        at = end;
    }
}

fn jpeg(bytes: &[u8]) -> bool {
    if !bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return false;
    }
    let body_end = bytes
        .iter()
        .rposition(|&byte| byte != 0)
        .map_or(0, |last| last + 1);
    bytes
        .get(..body_end)
        .is_some_and(|body| body.ends_with(&[0xFF, 0xD9]))
}

fn gif(bytes: &[u8]) -> bool {
    (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"))
        && bytes.len() >= 14
        && bytes.last() == Some(&0x3B)
}

fn webp(bytes: &[u8]) -> bool {
    let (Some(riff), Some(form), Some(size)) =
        (span(bytes, 0, 4), span(bytes, 8, 4), le_u32(bytes, 4))
    else {
        return false;
    };
    riff == b"RIFF"
        && form == b"WEBP"
        && u64::from(size).checked_add(8) == u64::try_from(bytes.len()).ok()
}

fn pdf(bytes: &[u8]) -> bool {
    let head = bytes.get(..1024).unwrap_or(bytes);
    let tail = bytes
        .get(bytes.len().saturating_sub(1024)..)
        .unwrap_or_default();
    contains(head, b"%PDF-") && contains(tail, b"%%EOF")
}

fn svg(bytes: &[u8]) -> bool {
    let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    std::str::from_utf8(text).is_ok_and(|text| text.contains("<svg"))
}

/// TrueType or OpenType: the table directory must fit, every table must lie
/// inside the file, and the file may carry at most the padding to a multiple
/// of four after the last table.
fn sfnt(bytes: &[u8]) -> bool {
    let tag_ok = bytes.starts_with(&[0x00, 0x01, 0x00, 0x00])
        || bytes.starts_with(b"true")
        || bytes.starts_with(b"OTTO");
    if !tag_ok || bytes.len() < 12 {
        return false;
    }
    let Some(count) = be_u16(bytes, 4).map(usize::from) else {
        return false;
    };
    let Some(directory_end) = count.checked_mul(16).and_then(|n| n.checked_add(12)) else {
        return false;
    };
    if directory_end > bytes.len() {
        return false;
    }
    let mut end = 0usize;
    for record in 0..count {
        let base = 12 + record * 16;
        let (Some(offset), Some(length)) = (be_u32(bytes, base + 8), be_u32(bytes, base + 12))
        else {
            return false;
        };
        let Some(stop) = (offset as usize).checked_add(length as usize) else {
            return false;
        };
        end = end.max(stop);
    }
    if end > bytes.len() {
        return false;
    }
    let Some(padded_end) = end.checked_add(3).map(|n| n & !3) else {
        return false;
    };
    bytes.len() <= padded_end
}

fn woff(bytes: &[u8]) -> bool {
    let tag_ok = bytes.starts_with(b"wOFF") || bytes.starts_with(b"wOF2");
    tag_ok
        && bytes.len() >= 12
        && be_u32(bytes, 8).and_then(|total| usize::try_from(total).ok()) == Some(bytes.len())
}

#[cfg(test)]
mod tests {
    use super::check_asset;

    fn png(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut out = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        for (kind, data) in chunks {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(*kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&[0; 4]);
        }
        out
    }

    fn ihdr(width: u32) -> Vec<u8> {
        let mut data = width.to_be_bytes().to_vec();
        data.extend_from_slice(&[0, 0, 0, 1, 8, 6, 0, 0, 0]);
        data
    }

    fn minimal_png(width: u32) -> Vec<u8> {
        png(&[(b"IHDR", ihdr(width)), (b"IEND", Vec::new())])
    }

    fn webp(body: &[u8]) -> Vec<u8> {
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(4 + body.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend_from_slice(body);
        out
    }

    /// A font with one table record at `offset` of `length`, padded to `total`.
    fn font(magic: &[u8; 4], offset: u32, length: u32, total: usize) -> Vec<u8> {
        let mut out = magic.to_vec();
        out.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(b"glyf");
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&offset.to_be_bytes());
        out.extend_from_slice(&length.to_be_bytes());
        out.resize(total, 0);
        out
    }

    fn woff(total: u32) -> Vec<u8> {
        let mut out = b"wOFF".to_vec();
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&total.to_be_bytes());
        out.resize(total as usize, 0);
        out
    }

    #[test]
    fn accepts_each_format() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[0; 7]);
        gif.push(0x3B);
        let samples: Vec<Vec<u8>> = vec![
            minimal_png(1),
            vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0xFF, 0xD9],
            gif,
            webp(b"VP8L\x00\x00\x00\x00"),
            b"%PDF-1.4\n1 0 obj\nendobj\n%%EOF\n".to_vec(),
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(),
            font(&[0, 1, 0, 0], 28, 4, 32),
            font(b"true", 28, 4, 32),
            font(b"OTTO", 28, 4, 32),
            woff(12),
        ];
        for sample in samples {
            assert!(check_asset(&sample).is_ok(), "refused {sample:?}");
        }
    }

    #[test]
    fn accepts_the_tutorial_icon() {
        let icon = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/examples/tutorial-html/librepaper-icon.png"
        ));
        assert!(check_asset(icon).is_ok());
    }

    #[test]
    fn refuses_empty_input() {
        assert!(check_asset(&[]).is_err());
    }

    #[test]
    fn refuses_a_zip() {
        let mut zip = b"PK\x03\x04".to_vec();
        zip.extend_from_slice(&[0; 64]);
        assert!(check_asset(&zip).is_err());
    }

    #[test]
    fn refuses_a_png_with_extra_bytes() {
        let mut bytes = minimal_png(1);
        bytes.push(0);
        assert!(check_asset(&bytes).is_err());
    }

    #[test]
    fn refuses_a_png_cut_inside_a_chunk() {
        let mut bytes = minimal_png(1);
        bytes.truncate(bytes.len() - 6);
        assert!(check_asset(&bytes).is_err());
    }

    #[test]
    fn refuses_a_png_chunk_length_of_u32_max() {
        let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(&[0xFF; 4]);
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&[0; 16]);
        assert!(check_asset(&bytes).is_err());
    }

    #[test]
    fn accepts_a_jpeg_with_trailing_zeros() {
        let bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0xFF, 0xD9, 0, 0, 0];
        assert!(check_asset(&bytes).is_ok());
    }

    #[test]
    fn refuses_a_webp_with_a_wrong_riff_size() {
        let mut bytes = webp(b"VP8L\x00\x00\x00\x00");
        bytes[4] = bytes[4].wrapping_add(1);
        assert!(check_asset(&bytes).is_err());
    }

    #[test]
    fn refuses_a_pdf_without_eof() {
        let bytes = b"%PDF-1.4\n1 0 obj\nendobj\n".to_vec();
        assert!(check_asset(&bytes).is_err());
    }

    #[test]
    fn refuses_invalid_utf8_svg() {
        let bytes = b"<svg \xFF\xFE></svg>".to_vec();
        assert!(check_asset(&bytes).is_err());
    }

    #[test]
    fn refuses_a_font_whose_table_runs_past_the_end() {
        let bytes = font(&[0, 1, 0, 0], 28, 100, 32);
        assert!(check_asset(&bytes).is_err());
    }

    #[test]
    fn refuses_a_woff_whose_length_disagrees() {
        let mut bytes = woff(32);
        bytes[11] = 33;
        assert!(check_asset(&bytes).is_err());
    }
}
