//! The small part of the tar format a backup needs: regular files with `/`-separated names, in
//! the POSIX ustar layout every `tar` understands. No links, devices or extended headers are
//! written, and none are honoured when reading: a backup is only ever files.

use std::io::{self, Read, Write};

const BLOCK: usize = 512;

fn put(block: &mut [u8; BLOCK], at: usize, len: usize, text: &[u8]) {
    block[at..at + text.len().min(len)].copy_from_slice(&text[..text.len().min(len)]);
}

fn octal(block: &mut [u8; BLOCK], at: usize, len: usize, value: u64) {
    // `len - 1` digits and a NUL.
    let text = format!("{value:0width$o}", width = len - 1);
    put(block, at, len - 1, text.as_bytes());
}

/// Splits a name over the ustar `prefix` and `name` fields when it is longer than 100 bytes.
fn split_name(name: &str) -> io::Result<(&str, &str)> {
    if name.len() <= 100 {
        return Ok(("", name));
    }
    for (i, _) in name.match_indices('/') {
        let (prefix, rest) = (&name[..i], &name[i + 1..]);
        if prefix.len() <= 155 && !rest.is_empty() && rest.len() <= 100 {
            return Ok((prefix, rest));
        }
    }
    Err(io::Error::new(io::ErrorKind::InvalidInput, format!("file name too long for a backup: {name}")))
}

/// Writes a tar stream.
pub(super) struct Writer<W: Write> {
    out: W,
}

impl<W: Write> Writer<W> {
    pub(super) fn new(out: W) -> Self {
        Self { out }
    }

    /// Adds a file whose bytes come from `src`; exactly `size` of them must be there.
    pub(super) fn append(&mut self, name: &str, size: u64, mtime: u64, mut src: impl Read) -> io::Result<()> {
        let (prefix, short) = split_name(name)?;
        let mut h = [0u8; BLOCK];
        put(&mut h, 0, 100, short.as_bytes());
        put(&mut h, 100, 8, b"0000600\0");
        put(&mut h, 108, 8, b"0000000\0");
        put(&mut h, 116, 8, b"0000000\0");
        if size < 8u64.pow(11) {
            octal(&mut h, 124, 12, size);
        } else {
            // Larger than 8 GiB: the GNU extension, a big-endian number with the top bit set.
            h[124] = 0x80;
            h[128..136].copy_from_slice(&size.to_be_bytes());
        }
        octal(&mut h, 136, 12, mtime);
        h[156] = b'0';
        put(&mut h, 257, 6, b"ustar\0");
        put(&mut h, 263, 2, b"00");
        put(&mut h, 345, 155, prefix.as_bytes());
        // The checksum is taken with its own field read as spaces.
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
        let text = format!("{sum:06o}\0 ");
        h[148..156].copy_from_slice(text.as_bytes());
        self.out.write_all(&h)?;

        let copied = io::copy(&mut src.by_ref().take(size), &mut self.out)?;
        if copied != size {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("{name} changed while it was read")));
        }
        let pad = (BLOCK - (size % BLOCK as u64) as usize) % BLOCK;
        self.out.write_all(&vec![0u8; pad])
    }

    pub(super) fn append_bytes(&mut self, name: &str, bytes: &[u8], mtime: u64) -> io::Result<()> {
        self.append(name, bytes.len() as u64, mtime, bytes)
    }

    /// Writes the end marker and returns the underlying writer.
    pub(super) fn finish(mut self) -> io::Result<W> {
        self.out.write_all(&[0u8; BLOCK * 2])?;
        Ok(self.out)
    }
}

/// One entry of a tar stream.
pub(super) struct Entry {
    pub(super) name: String,
    pub(super) size: u64,
    pub(super) is_file: bool,
}

/// Reads a tar stream, entry by entry.
pub(super) struct Reader<R: Read> {
    src: R,
    /// Bytes of the current entry (and its padding) not yet read or skipped.
    left: u64,
    done: bool,
}

fn parse_number(field: &[u8]) -> Option<u64> {
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        // GNU base-256: the first byte's top bit marks it, the rest is big-endian.
        let mut v = u64::from(field[0] & 0x7f);
        for b in &field[1..] {
            v = v.checked_mul(256)?.checked_add(u64::from(*b))?;
        }
        return Some(v);
    }
    let text: String = field.iter().take_while(|b| **b != 0).map(|b| char::from(*b)).collect();
    let text = text.trim();
    if text.is_empty() { Some(0) } else { u64::from_str_radix(text, 8).ok() }
}

fn cstr(field: &[u8]) -> String {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

impl<R: Read> Reader<R> {
    pub(super) fn new(src: R) -> Self {
        Self { src, left: 0, done: false }
    }

    /// Skips what is left of the previous entry and reads the next header. `None` at the end.
    pub(super) fn next_entry(&mut self) -> io::Result<Option<Entry>> {
        if self.done {
            return Ok(None);
        }
        io::copy(&mut self.src.by_ref().take(self.left), &mut io::sink())?;
        self.left = 0;
        let mut h = [0u8; BLOCK];
        if let Err(e) = self.src.read_exact(&mut h) {
            return if e.kind() == io::ErrorKind::UnexpectedEof { Ok(None) } else { Err(e) };
        }
        if h.iter().all(|b| *b == 0) {
            self.done = true;
            return Ok(None);
        }
        let want = parse_number(&h[148..156]).ok_or_else(|| bad("the checksum"))?;
        let mut copy = h;
        copy[148..156].copy_from_slice(b"        ");
        let sum: u64 = copy.iter().map(|b| u64::from(*b)).sum();
        if sum != want {
            return Err(bad("a header (the checksum does not match)"));
        }
        let size = parse_number(&h[124..136]).ok_or_else(|| bad("a size"))?;
        let prefix = cstr(&h[345..500]);
        let name = cstr(&h[0..100]);
        let name = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let kind = h[156];
        self.left =
            size.checked_add(((BLOCK as u64) - size % BLOCK as u64) % BLOCK as u64).ok_or_else(|| bad("a size"))?;
        Ok(Some(Entry { name, size, is_file: kind == b'0' || kind == 0 }))
    }

    /// Copies the current entry's bytes to `out`.
    pub(super) fn read_body(&mut self, size: u64, out: &mut impl Write) -> io::Result<()> {
        let n = io::copy(&mut self.src.by_ref().take(size), out)?;
        if n != size {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the backup ends in the middle of a file"));
        }
        self.left -= size;
        Ok(())
    }
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("this is not a valid backup: {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip() {
        let long = format!("uploads/{}/file.bin", "d".repeat(120));
        let mut w = Writer::new(Vec::new());
        w.append_bytes("manifest.json", b"{}", 7).expect("write");
        w.append_bytes(&long, &vec![9u8; 1000], 7).expect("write");
        w.append_bytes("empty", b"", 7).expect("write");
        let bytes = w.finish().expect("finish");

        let mut r = Reader::new(&bytes[..]);
        let e = r.next_entry().expect("read").expect("entry");
        assert_eq!((e.name.as_str(), e.size, e.is_file), ("manifest.json", 2, true));
        let mut body = Vec::new();
        r.read_body(e.size, &mut body).expect("body");
        assert_eq!(body, b"{}");
        let e = r.next_entry().expect("read").expect("entry");
        assert_eq!((e.name.as_str(), e.size), (long.as_str(), 1000));
        // Not reading the body is fine: the next call skips it.
        let e = r.next_entry().expect("read").expect("entry");
        assert_eq!((e.name.as_str(), e.size), ("empty", 0));
        assert!(r.next_entry().expect("read").is_none());
    }

    #[test]
    fn a_damaged_header_is_refused() {
        let mut w = Writer::new(Vec::new());
        w.append_bytes("a", b"x", 0).expect("write");
        let mut bytes = w.finish().expect("finish");
        bytes[3] ^= 0xff;
        assert!(Reader::new(&bytes[..]).next_entry().is_err());
    }

    #[test]
    fn a_short_source_is_an_error() {
        let mut w = Writer::new(Vec::new());
        assert!(w.append("a", 10, 0, &b"abc"[..]).is_err());
    }
}
