//! Pictures for the phone: album art from Spotify's image server and the clip recorder's
//! thumbnails. Both arrive as JPEG written by someone else — a server, or an ffmpeg that
//! encoded what the game drew — and neither is passed on as it came: each is **decoded to
//! its pixels, scaled to fit a box, and encoded again** as a PNG of one fixed form (8-bit
//! RGB, not interlaced, no optional chunks). What reaches the phone, or lies next to a clip,
//! is pixels and nothing else: no metadata, no comments, no colour profile, no trailing
//! bytes, nothing of the original file's structure.
//!
//! Bounded throughout: the input size ([`MAX_INPUT_BYTES`]), the image's declared size
//! (checked by the decoder before it allocates, [`MAX_SIDE`]), and the output, which is at
//! most the box. [`fuzz`] runs the whole path on arbitrary bytes.

use std::io::Write;

use crate::error::{Error, Result};

/// The largest JPEG accepted (album art is 50–300 KB, a thumbnail less).
pub const MAX_INPUT_BYTES: usize = 4 << 20;
/// Neither side of a source image may be larger (Spotify's largest cover is 640×640).
pub const MAX_SIDE: usize = 4096;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// A PNG made here, and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u16,
    pub height: u16,
    pub png: Vec<u8>,
}

fn bad(why: impl Into<String>) -> Error {
    Error::Format(format!("picture: {}", why.into()))
}

/// The size an image of `width`×`height` gets in a box of `max_w`×`max_h`: the same aspect
/// ratio, never larger than it was, at least one pixel each way.
pub fn fit(width: u32, height: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (1, 1);
    }
    if width <= max_w && height <= max_h {
        return (width, height);
    }
    // Scale by the tighter of the two limits, in integers (no rounding up past the box).
    let (w, h) = (u64::from(width), u64::from(height));
    let (mw, mh) = (u64::from(max_w.max(1)), u64::from(max_h.max(1)));
    if w * mh >= h * mw {
        (max_w.max(1), ((h * mw) / w).max(1) as u32)
    } else {
        (((w * mh) / h).max(1) as u32, max_h.max(1))
    }
}

/// Decode a baseline or progressive JPEG to 8-bit RGB.
pub fn decode_jpeg(jpeg: &[u8]) -> Result<(usize, usize, Vec<u8>)> {
    use zune_jpeg::{
        JpegDecoder,
        zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions},
    };
    if jpeg.len() > MAX_INPUT_BYTES {
        return Err(bad(format!("{} bytes is more than a picture for the phone may be", jpeg.len())));
    }
    if !jpeg.starts_with(&[0xff, 0xd8]) {
        return Err(bad("not a JPEG file"));
    }
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(MAX_SIDE)
        .set_max_height(MAX_SIDE)
        .set_use_unsafe(false);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(jpeg), options);
    let pixels = decoder.decode().map_err(|e| bad(format!("undecodable JPEG ({e:?})")))?;
    let info = decoder.info().ok_or_else(|| bad("undecodable JPEG (no header)"))?;
    let (w, h) = (usize::from(info.width), usize::from(info.height));
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || pixels.len() != w * h * 3 {
        return Err(bad("the decoder returned an image of an unexpected size"));
    }
    Ok((w, h, pixels))
}

/// Scale RGB pixels down by averaging the source area each target pixel covers.
fn shrink(rgb: &[u8], w: usize, h: usize, nw: usize, nh: usize) -> Vec<u8> {
    if (nw, nh) == (w, h) {
        return rgb.to_vec();
    }
    let mut out = Vec::with_capacity(nw * nh * 3);
    for ty in 0..nh {
        let (y0, y1) = (ty * h / nh, ((ty + 1) * h / nh).max(ty * h / nh + 1).min(h));
        for tx in 0..nw {
            let (x0, x1) = (tx * w / nw, ((tx + 1) * w / nw).max(tx * w / nw + 1).min(w));
            let mut sum = [0u64; 3];
            for y in y0..y1 {
                let row = &rgb[(y * w + x0) * 3..(y * w + x1) * 3];
                for px in row.as_chunks::<3>().0 {
                    sum[0] += u64::from(px[0]);
                    sum[1] += u64::from(px[1]);
                    sum[2] += u64::from(px[2]);
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            out.extend(sum.iter().map(|s| ((s + n / 2) / n) as u8));
        }
    }
    out
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    let mut out = Vec::with_capacity(data.len() + 12);
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
    out
}

/// Encode RGB pixels as a PNG: 8-bit RGB, rows filtered with "sub", one block of pixel data.
pub fn encode_png(width: usize, height: usize, rgb: &[u8]) -> Result<Vec<u8>> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE || rgb.len() != width * height * 3 {
        return Err(bad("pixels and size do not agree"));
    }
    let stride = width * 3;
    let mut rows = Vec::with_capacity(height * (stride + 1));
    for row in rgb.chunks_exact(stride) {
        rows.push(1); // filter type "sub": each byte minus the one a pixel to its left
        for (i, byte) in row.iter().enumerate() {
            rows.push(byte.wrapping_sub(if i >= 3 { row[i - 3] } else { 0 }));
        }
    }
    let mut deflater = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(4));
    let compressed =
        deflater.write_all(&rows).and_then(|()| deflater.finish()).map_err(|e| bad(format!("encoding: {e}")))?;
    let mut header = (width as u32).to_be_bytes().to_vec();
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = PNG_SIGNATURE.to_vec();
    out.extend(chunk(b"IHDR", &header));
    out.extend(chunk(b"IDAT", &compressed));
    out.extend(chunk(b"IEND", b""));
    Ok(out)
}

/// A JPEG as a PNG that fits in `max_w`×`max_h`.
pub fn jpeg_to_png(jpeg: &[u8], max_w: u32, max_h: u32) -> Result<Picture> {
    let (w, h, rgb) = decode_jpeg(jpeg)?;
    let (nw, nh) = fit(w as u32, h as u32, max_w, max_h);
    let (nw, nh) = (nw as usize, nh as usize);
    let png = encode_png(nw, nh, &shrink(&rgb, w, h, nw, nh))?;
    Ok(Picture { width: nw as u16, height: nh as u16, png })
}

/// The size a PNG says it has, if it starts like a PNG with an intact header chunk. (For
/// pictures read back from disk, which another program may have replaced.)
pub fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    let rest = png.strip_prefix(&PNG_SIGNATURE)?;
    if rest.len() < 25 || rest[..8] != [0, 0, 0, 13, b'I', b'H', b'D', b'R'] {
        return None;
    }
    let data = &rest[8..21];
    let mut crc = flate2::Crc::new();
    crc.update(b"IHDR");
    crc.update(data);
    if crc.sum().to_be_bytes() != rest[21..25] {
        return None;
    }
    let w = u32::from_be_bytes(data[0..4].try_into().ok()?);
    let h = u32::from_be_bytes(data[4..8].try_into().ok()?);
    ((1..=MAX_SIDE as u32).contains(&w) && (1..=MAX_SIDE as u32).contains(&h)).then_some((w, h))
}

/// The whole path on arbitrary bytes: it returns, and what it makes fits its box and reads
/// back as the size it claims.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    if let Ok(picture) = jpeg_to_png(data, 320, 320) {
        assert!(picture.width >= 1 && picture.width <= 320 && picture.height >= 1 && picture.height <= 320);
        assert_eq!(png_dimensions(&picture.png), Some((u32::from(picture.width), u32::from(picture.height))));
    }
    let _ = png_dimensions(data);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A real JPEG, made by ffmpeg from a test pattern (`testsrc2`, 64×48, quality 5):
    /// what both callers receive in practice.
    pub(crate) fn sample_jpeg(width: u32, height: u32) -> Vec<u8> {
        let out = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg(format!("testsrc2=size={width}x{height}:rate=1"))
            .args(["-frames:v", "1", "-q:v", "5", "-f", "mjpeg", "-"])
            .output()
            .expect("ffmpeg");
        assert!(out.status.success() && out.stdout.starts_with(&[0xff, 0xd8]));
        out.stdout
    }

    #[test]
    fn the_box_keeps_the_aspect_ratio_and_never_enlarges() {
        assert_eq!(fit(640, 640, 320, 320), (320, 320));
        assert_eq!(fit(1280, 720, 320, 320), (320, 180));
        assert_eq!(fit(720, 1280, 320, 320), (180, 320));
        assert_eq!(fit(64, 64, 320, 320), (64, 64), "small pictures stay small");
        assert_eq!(fit(4096, 1, 320, 320), (320, 1), "at least a pixel");
        assert_eq!(fit(0, 10, 320, 320), (1, 1));
    }

    #[test]
    fn a_jpeg_becomes_a_png_that_fits_the_box() {
        let jpeg = sample_jpeg(640, 480);
        let picture = jpeg_to_png(&jpeg, 320, 320).unwrap();
        assert_eq!((picture.width, picture.height), (320, 240));
        assert_eq!(png_dimensions(&picture.png), Some((320, 240)));
        // The same pixels give the same file: nothing of the input but its pixels.
        assert_eq!(jpeg_to_png(&jpeg, 320, 320).unwrap(), picture);
        // Bytes after the image (where a payload could hide) change nothing.
        let mut padded = jpeg.clone();
        padded.extend_from_slice(b"PAYLOAD-AFTER-EOI");
        assert_eq!(jpeg_to_png(&padded, 320, 320).unwrap().png, picture.png);
        // A small picture is re-encoded at its own size.
        let small = jpeg_to_png(&sample_jpeg(64, 48), 320, 320).unwrap();
        assert_eq!((small.width, small.height), (64, 48));
    }

    #[test]
    fn shrinking_averages_what_each_pixel_covers() {
        // 2×2 of four colours → one pixel of their mean.
        let rgb = [0, 0, 0, 255, 255, 255, 100, 0, 0, 0, 100, 0];
        assert_eq!(shrink(&rgb, 2, 2, 1, 1), vec![89, 89, 64]);
        let png = encode_png(1, 1, &[1, 2, 3]).unwrap();
        assert_eq!(png_dimensions(&png), Some((1, 1)));
    }

    #[test]
    fn what_is_not_a_jpeg_is_refused() {
        for junk in [&b""[..], b"\xff\xd8", b"GIF89a", &[0xff, 0xd8, 0xff, 0xdb, 0, 3, 0], b"\x89PNG\r\n\x1a\n"] {
            assert!(jpeg_to_png(junk, 320, 320).is_err());
        }
        assert!(jpeg_to_png(&vec![0xff; MAX_INPUT_BYTES + 1], 320, 320).is_err());
        let jpeg = sample_jpeg(64, 48);
        for cut in [4, 20, jpeg.len() / 2] {
            let _ = jpeg_to_png(&jpeg[..cut], 320, 320); // returns; may or may not decode partly
        }
        // A damaged header chunk is not a size.
        let mut png = encode_png(2, 2, &[0; 12]).unwrap();
        png[20] ^= 1;
        assert_eq!(png_dimensions(&png), None);
    }
}
