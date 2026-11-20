//! Pure parts of the upload pipeline: sniffing, name cleaning, keys, ranges.

use super::*;

#[test]
fn every_allowed_type_is_recognised_by_its_magic_bytes() {
    let cases: [(&[u8], &str, &str, bool); 7] = [
        (b"\x89PNG\r\n\x1a\n....", "image/png", "png", true),
        (b"\xFF\xD8\xFF\xE0....", "image/jpeg", "jpg", true),
        (b"GIF87a....", "image/gif", "gif", true),
        (b"GIF89a....", "image/gif", "gif", true),
        (b"RIFF\x00\x00\x00\x00WEBPVP8 ", "image/webp", "webp", true),
        (b"%PDF-1.7\n", "application/pdf", "pdf", false),
        (b"PK\x03\x04rest", "application/zip", "zip", false),
    ];
    for (bytes, mime, ext, image) in cases {
        assert_eq!(sniff(bytes, "x"), Some((mime, ext, image)), "{mime}");
    }
}

#[test]
fn the_declared_name_never_decides_the_type() {
    // Extension/MIME mismatch: the bytes win, in both directions.
    assert_eq!(sniff(b"\x89PNG\r\n\x1a\n..", "photo.jpg").map(|s| s.1), Some("png"));
    assert_eq!(sniff(b"%PDF-1.4", "photo.png").map(|s| s.1), Some("pdf"));
    assert_eq!(
        sniff(b"PK\x03\x04", "x.png").map(|s| (s.1, s.2)),
        Some(("zip", false)),
        "a zip named .png is not an image"
    );
    assert!(sniff(b"GIF8", "a.gif").is_none(), "truncated magic");
    assert!(sniff(b"RIFF\x00\x00\x00\x00WAVEfmt ", "a.webp").is_none(), "RIFF but not WebP");
    assert!(sniff(b"RIFF", "a.webp").is_none(), "too short to be WebP");
    assert!(sniff(b"", "a.png").is_none());
    assert!(sniff(b"", "a.txt").is_some(), "an empty text is valid UTF-8 (the empty file is refused earlier)");
}

#[test]
fn active_content_is_never_allowed_through() {
    for (bytes, name) in [
        (&b"<svg xmlns='http://www.w3.org/2000/svg'><script>1</script></svg>"[..], "a.svg"),
        (b"<!DOCTYPE html><script>", "a.html"),
        (b"<?php system($_GET[0]);", "a.php"),
        (b"#!/bin/sh\nrm -rf /", "a.sh"),
        (b"MZ\x90\x00\x03", "a.exe"),
        (b"\x7fELF", "a.bin"),
        (b"GIF89", "a.gif"),
    ] {
        assert!(sniff(bytes, name).is_none(), "{name}");
    }
    // Plain text is only taken under a .txt name, valid UTF-8, no NULs, and is served as text.
    assert_eq!(sniff(b"hello", "NOTES.TXT").map(|s| s.0), Some("text/plain; charset=utf-8"));
    assert!(sniff(b"hello", "notes.txt.html").is_none());
    assert!(sniff(b"\xff\xfe bad utf8", "a.txt").is_none());
    assert!(sniff(b"a\0b", "a.txt").is_none());
}

#[test]
fn polyglot_image_prefix_is_still_classified_by_its_first_bytes() {
    // A PNG header followed by HTML is an image to the sniffer; serving it with the sniffed
    // MIME (never the name) is what keeps it inert. Document that the type is the PNG one.
    let poly = b"\x89PNG\r\n\x1a\n<script>alert(1)</script>";
    assert_eq!(sniff(poly, "x.html").map(|s| s.0), Some("image/png"));
}

#[test]
fn filenames_lose_paths_controls_and_reserved_characters() {
    assert_eq!(clean_filename("/etc/passwd"), "passwd");
    assert_eq!(clean_filename(r"..\..\boot.ini"), "boot.ini");
    assert_eq!(clean_filename("a/b/c/"), "file", "a trailing slash leaves nothing");
    assert_eq!(clean_filename("na\u{0}me\n.pdf"), "name.pdf");
    assert_eq!(clean_filename("we\"ird|na:me*?.zip"), "weirdname.zip");
    assert_eq!(clean_filename(""), "file");
    assert_eq!(clean_filename("<>"), "file");
    assert_eq!(clean_filename("zażółć gęślą.png"), "zażółć gęślą.png", "unicode is kept");
    assert_eq!(clean_filename(&"x".repeat(500)).chars().count(), 120);
    assert!(!clean_filename("a\r\nSet-Cookie: x=1").contains(['\r', '\n']), "no header injection");
}

#[test]
fn storage_keys_are_lowercase_sha_plus_a_known_extension() {
    let sha = "0123456789abcdef".repeat(4);
    for ext in ["png", "jpg", "gif", "webp", "pdf", "zip", "txt"] {
        assert!(valid_key(&format!("{sha}.{ext}")), "{ext}");
    }
    for bad in [
        format!("{sha}.svg"),
        format!("{sha}.html"),
        format!("{sha}."),
        sha.to_string(),
        format!("{}.png", &sha[1..]),
        format!("{sha}0.png"),
        format!("{}g.png", &sha[1..]),
        format!("../{sha}.png"),
        format!("{sha}.png/../x"),
        format!("{sha}.png.exe"),
        String::new(),
        ".png".to_owned(),
    ] {
        assert!(!valid_key(&bad), "{bad:?}");
    }
}

#[test]
fn thumbnail_keys_take_only_the_known_widths() {
    let sha = "a".repeat(64);
    let width = bf_cook::THUMB_WIDTHS[0];
    assert_eq!(thumb_key(&format!("{sha}-{width}.png")), Some((format!("{sha}.png"), width)));
    assert_eq!(
        thumb_key(&format!("{sha}-{}.png", width + 1)),
        None,
        "an arbitrary width would let anyone fill the disk"
    );
    assert_eq!(thumb_key(&format!("{sha}-abc.png")), None);
    assert_eq!(thumb_key(&format!("{sha}.png")), None);
    assert_eq!(thumb_key(&format!("{sha}--1.png")), None);
}

#[test]
fn byte_ranges_stay_inside_the_file() {
    assert_eq!(byte_range("bytes=0-9", 100), Some((0, 9)));
    assert_eq!(byte_range("bytes=90-", 100), Some((90, 99)));
    assert_eq!(byte_range("bytes=-10", 100), Some((90, 99)));
    assert_eq!(byte_range("bytes=-500", 100), Some((0, 99)), "a suffix longer than the file is the file");
    assert_eq!(byte_range("bytes=50-5000", 100), Some((50, 99)), "end is clamped");
    for bad in [
        "bytes=100-",
        "bytes=200-300",
        "bytes=9-0",
        "bytes=-0",
        "bytes=0-1,5-6",
        "bytes=a-b",
        "bytes=",
        "items=0-1",
        "0-1",
        "bytes=-",
    ] {
        assert_eq!(byte_range(bad, 100), None, "{bad}");
    }
    assert_eq!(byte_range("bytes=0-0", 0), None, "empty file");
    assert_eq!(byte_range("bytes=0-18446744073709551616", 100), None, "overflowing numbers do not parse");
}
