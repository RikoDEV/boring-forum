//! Image metadata scrubbing on upload (privacy): location never leaves the uploader's
//! device by way of this site. Always: the GPS block of a JPEG's EXIF is emptied, and XMP and
//! IPTC (which can carry places too) are dropped, as are PNG and WebP metadata chunks. With
//! `upload_strip_metadata` on, a JPEG keeps nothing but its orientation.
//!
//! Every offset is bounds-checked and nothing panics. The scrub fails closed: an image whose
//! structure cannot be followed (a truncated segment, a bad length) returns `None` and is
//! rejected, because storing it as it came would keep its location.

/// The bytes to store for an upload of this type, or `None` if it cannot be cleaned.
pub(crate) fn scrub(mime: &str, bytes: &[u8], strip_all: bool) -> Option<Vec<u8>> {
    match mime {
        "image/jpeg" => jpeg(bytes, strip_all),
        "image/png" => png(bytes),
        "image/webp" => webp(bytes),
        _ => Some(bytes.to_vec()),
    }
}

fn be16(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 2).map(|s| usize::from(u16::from_be_bytes([s[0], s[1]])))
}

fn jpeg(b: &[u8], strip_all: bool) -> Option<Vec<u8>> {
    if !b.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&b[..2]);
    let mut i = 2;
    loop {
        if i + 4 > b.len() || b[i] != 0xFF {
            // Not a segment where one was expected: keep the rest as it is.
            out.extend_from_slice(b.get(i..)?);
            return Some(out);
        }
        let marker = b[i + 1];
        // Fill bytes before a marker: skip one and look again.
        if marker == 0xFF {
            out.push(0xFF);
            i += 1;
            continue;
        }
        // End of image: whatever follows (a second picture with its own EXIF) is not kept.
        if marker == 0xD9 {
            out.extend_from_slice(&b[i..i + 2]);
            return Some(out);
        }
        // Start of scan: compressed data follows, with no more metadata.
        if marker == 0xDA {
            out.extend_from_slice(&b[i..]);
            return Some(out);
        }
        // Markers without a length.
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            out.extend_from_slice(&b[i..i + 2]);
            i += 2;
            continue;
        }
        let len = be16(b, i + 2)?;
        if len < 2 {
            return None;
        }
        let end = i.checked_add(2 + len)?;
        let seg = b.get(i..end)?;
        let body = &seg[4..];
        match marker {
            0xE1 if body.starts_with(b"Exif\0\0") => {
                let tiff = &body[6..];
                let cleaned = if strip_all { orientation_only(tiff) } else { without_gps(tiff) };
                if let Some(t) = cleaned {
                    let total = t.len() + 6 + 2;
                    if let Ok(l) = u16::try_from(total) {
                        out.extend_from_slice(&[0xFF, 0xE1]);
                        out.extend_from_slice(&l.to_be_bytes());
                        out.extend_from_slice(b"Exif\0\0");
                        out.extend_from_slice(&t);
                    }
                }
            }
            // XMP (and extended XMP) in APP1, IPTC in APP13: dropped.
            0xE1 | 0xED => {}
            _ => out.extend_from_slice(seg),
        }
        i = end;
    }
}

struct Tiff {
    b: Vec<u8>,
    le: bool,
}

impl Tiff {
    fn u16(&self, at: usize) -> Option<u16> {
        let s = self.b.get(at..at + 2)?;
        Some(if self.le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) })
    }
    fn u32(&self, at: usize) -> Option<u32> {
        let s = self.b.get(at..at + 4)?;
        let a = [s[0], s[1], s[2], s[3]];
        Some(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }
}

fn header(tiff: &[u8]) -> Option<Tiff> {
    let le = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let t = Tiff { b: tiff.to_vec(), le };
    (t.u16(2)? == 42).then_some(t)
}

/// Bytes one value of a TIFF type takes.
fn type_size(ty: u16) -> usize {
    match ty {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

/// The TIFF block with the GPS directory emptied: its entries and the values they point at are
/// zeroed, and its count set to 0. A broken GPS directory drops the whole block.
fn without_gps(tiff: &[u8]) -> Option<Vec<u8>> {
    let mut t = header(tiff)?;
    let ifd0 = usize::try_from(t.u32(4)?).ok()?;
    let n = usize::from(t.u16(ifd0)?);
    let mut gps = None;
    for k in 0..n.min(500) {
        let e = ifd0 + 2 + k * 12;
        if t.u16(e)? == 0x8825 {
            gps = Some(usize::try_from(t.u32(e + 8)?).ok()?);
        }
    }
    let Some(g) = gps else { return Some(t.b) };
    let count = usize::from(t.u16(g)?);
    let dir_end = g.checked_add(2 + count.checked_mul(12)?)?;
    if dir_end > t.b.len() {
        return None;
    }
    for k in 0..count {
        let e = g + 2 + k * 12;
        let size = type_size(t.u16(e + 2)?).checked_mul(usize::try_from(t.u32(e + 4)?).ok()?)?;
        if size > 4 {
            let off = usize::try_from(t.u32(e + 8)?).ok()?;
            let end = off.checked_add(size)?.min(t.b.len());
            if off < end {
                t.b[off..end].fill(0);
            }
        }
    }
    t.b[g..dir_end].fill(0);
    Some(t.b)
}

/// A new TIFF block holding only the orientation, when the image has one.
fn orientation_only(tiff: &[u8]) -> Option<Vec<u8>> {
    let t = header(tiff)?;
    let ifd0 = usize::try_from(t.u32(4)?).ok()?;
    let n = usize::from(t.u16(ifd0)?);
    let mut orientation = None;
    for k in 0..n.min(500) {
        let e = ifd0 + 2 + k * 12;
        if t.u16(e)? == 0x0112 {
            orientation = t.u16(e + 8);
        }
    }
    let o = orientation.filter(|o| (1..=8).contains(o))?;
    let mut out = b"MM\0\x2A\0\0\0\x08".to_vec();
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0x0112u16.to_be_bytes());
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&o.to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&0u32.to_be_bytes());
    Some(out)
}

/// PNG without `eXIf` and without text chunks holding XMP.
fn png(b: &[u8]) -> Option<Vec<u8>> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !b.starts_with(SIG) {
        return None;
    }
    let mut out = SIG.to_vec();
    let mut i = SIG.len();
    while i + 12 <= b.len() {
        let len = usize::try_from(u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])).ok()?;
        let end = i.checked_add(12)?.checked_add(len)?;
        let chunk = b.get(i..end)?;
        let kind = &chunk[4..8];
        let data = &chunk[8..8 + len];
        let xmp = matches!(kind, b"iTXt" | b"tEXt" | b"zTXt") && data.starts_with(b"XML:com.adobe.xmp");
        if kind != b"eXIf" && !xmp {
            out.extend_from_slice(chunk);
        }
        i = end;
        if kind == b"IEND" {
            break;
        }
    }
    Some(out)
}

/// WebP without its EXIF and XMP chunks, with the sizes and feature flags updated.
fn webp(b: &[u8]) -> Option<Vec<u8>> {
    if b.len() < 12 || &b[..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return None;
    }
    let mut body = Vec::with_capacity(b.len());
    let mut i = 12;
    let mut vp8x_at = None;
    while i + 8 <= b.len() {
        let kind = &b[i..i + 4];
        let len = usize::try_from(u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]])).ok()?;
        let padded = len + (len & 1);
        let end = i.checked_add(8)?.checked_add(padded)?.min(b.len());
        if kind != b"EXIF" && kind != b"XMP " {
            if kind == b"VP8X" {
                vp8x_at = Some(body.len());
            }
            body.extend_from_slice(&b[i..end]);
        }
        i = end;
    }
    // VP8X flags: bit 3 is EXIF, bit 2 is XMP.
    if let Some(at) = vp8x_at
        && let Some(flags) = body.get_mut(at + 8)
    {
        *flags &= !0b0000_1100;
    }
    let size = u32::try_from(body.len() + 4).ok()?;
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IFD0 with one GPS pointer, and a GPS IFD with a latitude (rational x3) stored outside.
    fn tiff_with_gps() -> Vec<u8> {
        let mut t = b"MM\0\x2A\0\0\0\x08".to_vec();
        t.extend_from_slice(&1u16.to_be_bytes()); // IFD0 at 8: one entry
        t.extend_from_slice(&0x8825u16.to_be_bytes());
        t.extend_from_slice(&4u16.to_be_bytes());
        t.extend_from_slice(&1u32.to_be_bytes());
        t.extend_from_slice(&26u32.to_be_bytes()); // GPS IFD at 26
        t.extend_from_slice(&0u32.to_be_bytes()); // next IFD
        t.extend_from_slice(&1u16.to_be_bytes()); // GPS IFD: one entry
        t.extend_from_slice(&2u16.to_be_bytes()); // GPSLatitude
        t.extend_from_slice(&5u16.to_be_bytes());
        t.extend_from_slice(&3u32.to_be_bytes());
        t.extend_from_slice(&44u32.to_be_bytes()); // values at 44
        t.extend_from_slice(&0u32.to_be_bytes());
        for v in [52u32, 1, 13, 1, 7, 1] {
            t.extend_from_slice(&v.to_be_bytes());
        }
        t
    }

    fn jpeg_with(tiff: &[u8]) -> Vec<u8> {
        let mut seg = b"Exif\0\0".to_vec();
        seg.extend_from_slice(tiff);
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&((seg.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&seg);
        // An XMP segment that must go.
        let xmp = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>GPS</x:xmpmeta>";
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((xmp.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(xmp);
        out.extend_from_slice(&[0xFF, 0xDA, 0, 2, 1, 2, 3, 0xFF, 0xD9]);
        out
    }

    #[test]
    fn gps_values_are_gone() {
        let input = jpeg_with(&tiff_with_gps());
        let out = scrub("image/jpeg", &input, false).unwrap();
        let needle: Vec<u8> = 52u32.to_be_bytes().iter().chain(1u32.to_be_bytes().iter()).copied().collect();
        assert!(input.windows(8).any(|w| w == needle.as_slice()));
        assert!(!out.windows(8).any(|w| w == needle.as_slice()), "latitude still present");
        assert!(!out.windows(3).any(|w| w == b"GPS"), "XMP still present");
        assert!(out.ends_with(&[0xFF, 0xDA, 0, 2, 1, 2, 3, 0xFF, 0xD9]), "image data changed");
    }

    #[test]
    fn strip_all_keeps_only_orientation() {
        let mut t = b"II\x2A\0\x08\0\0\0".to_vec();
        t.extend_from_slice(&1u16.to_le_bytes());
        t.extend_from_slice(&0x0112u16.to_le_bytes());
        t.extend_from_slice(&3u16.to_le_bytes());
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(&6u16.to_le_bytes());
        t.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let out = scrub("image/jpeg", &jpeg_with(&t), true).unwrap();
        assert!(out.windows(2).any(|w| w == [0x01, 0x12]), "orientation kept");
    }

    #[test]
    fn malformed_jpegs_with_location_are_rejected_not_stored_as_they_came() {
        let latitude: Vec<u8> = 52u32.to_be_bytes().iter().chain(1u32.to_be_bytes().iter()).copied().collect();
        let good = jpeg_with(&tiff_with_gps());
        let mut cases: Vec<Vec<u8>> = Vec::new();
        // Cut inside the EXIF segment.
        cases.push(good[..40].to_vec());
        // The XMP segment after the EXIF one claims more bytes than the file has.
        let mut long = good.clone();
        let xmp_at = long.windows(2).rposition(|w| w == [0xFF, 0xE1]).unwrap();
        long[xmp_at + 2] = 0xFF;
        cases.push(long);
        // A segment length below 2 after the EXIF one.
        let mut short = good.clone();
        short[xmp_at + 2] = 0;
        short[xmp_at + 3] = 1;
        cases.push(short);
        // A length that cannot be read: the file ends right after a marker.
        let mut cut = good[..xmp_at].to_vec();
        cut.extend_from_slice(&[0xFF, 0xE1, 0xFF]);
        cases.push(cut);
        for case in cases {
            match scrub("image/jpeg", &case, false) {
                None => {}
                Some(out) => assert!(!out.windows(8).any(|w| w == latitude.as_slice()), "location kept: {case:?}"),
            }
        }
        // Fill bytes between segments are legal and do not fail the scrub.
        let mut filled = good[..2].to_vec();
        filled.extend_from_slice(&[0xFF, 0xFF]);
        filled.extend_from_slice(&good[2..]);
        let out = scrub("image/jpeg", &filled, false).unwrap();
        assert!(!out.windows(8).any(|w| w == latitude.as_slice()));
        // Bytes after the end of image are dropped.
        let mut trailer = vec![0xFF, 0xD8, 0xFF, 0xD9];
        trailer.extend_from_slice(&good);
        assert_eq!(scrub("image/jpeg", &trailer, false).unwrap(), [0xFF, 0xD8, 0xFF, 0xD9]);
    }

    #[test]
    fn broken_files_never_panic() {
        for bad in [
            &b"\xFF\xD8\xFF\xE1\xFF\xFF"[..],
            b"\xFF\xD8\xFF\xE1\x00\x01",
            b"\x89PNG\r\n\x1a\n\xff\xff\xff\xff",
            b"RIFF\0\0\0\0WEBPEXIF\xff\xff\xff\xff",
        ] {
            let _ = scrub("image/jpeg", bad, false);
            let _ = scrub("image/png", bad, false);
            let _ = scrub("image/webp", bad, false);
        }
    }
}
