//! Clearing the pixels of an image under redaction regions. The image is decoded, every pixel
//! whose footprint intersects a region is set to zero (no ink for image masks), and the result is
//! written as a new Flate image; the original object is untouched (other pages may use it).
//! Images whose codec PdfCraft can't decode (DCT, JPX, JBIG2, CCITT) return `None`, and the
//! caller removes the whole image instead; that is fail-closed.

use pdfcraft_content::{Matrix, overlaps};
use pdfcraft_cos::{Document, Object, Stream};

/// Most pixels an image may have to be cleared in place (larger ones are removed).
const MAX_PIXELS: u64 = 64 << 20;

fn components(doc: &Document, s: &Stream) -> Option<usize> {
    let cs = doc.resolve(s.dict.get(b"ColorSpace")?);
    let cs = &*cs;
    let name = match cs {
        Object::Name(n) => n.as_slice(),
        Object::Array(a) => a.first()?.as_name()?,
        _ => return None,
    };
    Some(match name {
        b"DeviceGray" | b"G" | b"CalGray" | b"Indexed" | b"I" | b"Separation" => 1,
        b"DeviceRGB" | b"RGB" | b"CalRGB" | b"Lab" => 3,
        b"DeviceCMYK" | b"CMYK" => 4,
        b"ICCBased" => {
            // /N lives in the ICC profile stream, and shall be 1, 3 or 4 (ISO 32000-2 §8.6.5.5).
            match &*doc.resolve(cs.as_array()?.get(1)?) {
                Object::Stream(icc) => usize::try_from(icc.dict.int(b"N")?).ok().filter(|n| matches!(n, 1 | 3 | 4))?,
                _ => return None,
            }
        }
        _ => return None,
    })
}

/// `Ok(Some(copy))` with the covered pixels cleared, `Ok(None)` when no pixel intersects,
/// `Err(())` when the image can't be cleared (the caller removes it).
#[allow(clippy::result_unit_err)]
pub(crate) fn clear(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]]) -> Result<Option<Stream>, ()> {
    decode_and_clear(doc, s, ctm, rects).ok_or(())
}

fn decode_and_clear(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]]) -> Option<Option<Stream>> {
    // Separate masks can retain the original image data.
    if s.dict.contains(b"SMask") || s.dict.contains(b"Mask") {
        return None;
    }
    let supported = |n: &[u8]| {
        matches!(
            n,
            b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW" | b"ASCII85Decode" | b"A85" | b"ASCIIHexDecode" | b"AHx" | b"RunLengthDecode" | b"RL"
        )
    };
    let ok = match s.dict.get(b"Filter") {
        None => true,
        Some(Object::Name(n)) => supported(n),
        Some(Object::Array(a)) => a.iter().all(|f| f.as_name().is_some_and(supported)),
        _ => false,
    };
    if !ok {
        return None;
    }
    let (w, h) = (s.dict.int(b"Width")?, s.dict.int(b"Height")?);
    if w <= 0 || h <= 0 || (w as u64) * (h as u64) > MAX_PIXELS {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let mask = matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true)));
    let (ncomp, bpc) = if mask { (1, 1) } else { (components(doc, s)?, s.dict.int(b"BitsPerComponent")? as usize) };
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
        return None;
    }
    // Every size comes from the file: multiply checked, so an overflowing product refuses the image
    // instead of sizing the writes below.
    let bits_per_pixel = ncomp.checked_mul(bpc)?;
    let row = w.checked_mul(bits_per_pixel)?.div_ceil(8);
    let bytes = row.checked_mul(h)?;
    let row_bits = row.checked_mul(8)?;
    bytes.checked_mul(8)?;
    // Decode no further than the raster can need: a filter stage may legitimately produce up to
    // twice it (ASCIIHex text under Flate) plus a PNG predictor's byte per row. Data past that
    // can only be a decompression bomb; the image is then removed whole.
    let allowance = bytes.checked_add(h)?.checked_mul(2)?.checked_add(4096)?;
    let mut data = s.decoded_within(allowance).ok()?;
    if data.len() < bytes {
        return None;
    }
    data.truncate(bytes);
    // Image masks: a sample of 0 paints unless /Decode is [1 0]; cleared samples must not paint.
    let clear_bit = if mask {
        let inverted = s.dict.get(b"Decode").and_then(Object::as_array).and_then(|d| d.first()).and_then(Object::as_f64) == Some(1.0);
        !inverted
    } else {
        false
    };
    let mut cleared = 0usize;
    // The overlapping columns of a row are one run per region: every pixel box moves
    // monotonically with the column. Regions' runs are merged, and whole bytes of them are
    // filled at once; only the two edges of a run are bit by bit.
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for j in 0..h {
        // Image space has its origin at the bottom left; row 0 is at the top.
        let v0 = 1.0 - (j + 1) as f64 / h as f64;
        let v1 = 1.0 - j as f64 / h as f64;
        let row_box = ctm.bbox([0.0, v0, 1.0, v1]);
        if !rects.iter().any(|r| overlaps(*r, row_box, 0.0)) {
            continue;
        }
        spans.clear();
        for r in rects {
            if let Some(span) = covered_columns(ctm, *r, v0, v1, w) {
                spans.push(span);
            }
        }
        if spans.is_empty() {
            continue;
        }
        spans.sort_unstable();
        let (mut lo, mut hi) = spans[0];
        for span in spans.iter().copied().skip(1) {
            if span.0 <= hi + 1 {
                hi = hi.max(span.1);
                continue;
            }
            cleared += hi - lo + 1;
            clear_columns(&mut data, j * row_bits + lo * bits_per_pixel, j * row_bits + (hi + 1) * bits_per_pixel, clear_bit)?;
            (lo, hi) = span;
        }
        cleared += hi - lo + 1;
        clear_columns(&mut data, j * row_bits + lo * bits_per_pixel, j * row_bits + (hi + 1) * bits_per_pixel, clear_bit)?;
    }
    if cleared == 0 {
        return Some(None);
    }
    let mut dict = s.dict.clone();
    dict.remove(b"Length");
    dict.remove(b"DecodeParms");
    dict.remove(b"Alternates");
    Some(Some(Stream::flate(dict, &data)))
}

/// The first and last column of a row whose pixel box overlaps `r`, or `None` when no pixel
/// does. The box of a pixel moves monotonically with the column, so the columns passing the
/// exact per-pixel test form one run: found from that test's four comparisons without ever
/// transforming a pixel twice.
fn covered_columns(ctm: &Matrix, r: [f64; 4], v0: f64, v1: f64, w: usize) -> Option<(usize, usize)> {
    // A region with no width or height never overlaps (the test is strict), and a transform
    // that maps every pixel box onto a line or point never does either.
    let [a, b, c, d, _, _] = ctm.0;
    if !(r[0] < r[2] && r[1] < r[3]) || (a == 0.0 && c == 0.0) || (b == 0.0 && d == 0.0) {
        return None;
    }
    let pixel_box = |i: usize| ctm.bbox([i as f64 / w as f64, v0, (i + 1) as f64 / w as f64, v1]);
    let (mut lo, mut hi) = (0, w.checked_sub(1)?);
    for run in [
        true_run(|i| pixel_box(i)[2] > r[0], w),
        true_run(|i| pixel_box(i)[0] < r[2], w),
        true_run(|i| pixel_box(i)[3] > r[1], w),
        true_run(|i| pixel_box(i)[1] < r[3], w),
    ] {
        let (first, last) = run?;
        lo = lo.max(first);
        hi = hi.min(last);
    }
    (lo <= hi).then_some((lo, hi))
}

/// The columns where the monotone predicate `p` holds, which is one run of them (the pixel box
/// comparisons it stands for only move one way with the column).
fn true_run(p: impl Fn(usize) -> bool, w: usize) -> Option<(usize, usize)> {
    if p(0) {
        if p(w - 1) {
            return Some((0, w - 1));
        }
        // True first, then false: find where it turns.
        let (mut lo, mut hi) = (0, w - 1);
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if p(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some((0, lo))
    } else {
        if !p(w - 1) {
            return None;
        }
        // False first, then true: find where it turns.
        let (mut lo, mut hi) = (0, w - 1);
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if p(mid) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        Some((hi, w - 1))
    }
}

/// Set (an inverted mask's cleared sample) or clear every bit in `[start, end)`, whole bytes at
/// a time where a byte is covered. `bytes * 8` fits and every index is below it, so this
/// arithmetic can't overflow; a byte outside the data refuses the image.
fn clear_columns(data: &mut [u8], start: usize, end: usize, set: bool) -> Option<()> {
    let mut bit = start;
    // A partial first byte is the only one bit by bit, up to the last whole byte.
    if !bit.is_multiple_of(8) {
        let take = (8 - bit % 8).min(end - bit);
        for b in bit..bit + take {
            let (byte, shift) = (b / 8, 7 - b % 8);
            let v = data.get_mut(byte)?;
            if set {
                *v |= 1 << shift;
            } else {
                *v &= !(1 << shift);
            }
        }
        bit += take;
    }
    let whole = (end - bit) / 8 * 8;
    if whole > 0 {
        data.get_mut(bit / 8..(bit + whole) / 8)?.fill(if set { 0xFF } else { 0x00 });
        bit += whole;
    }
    for b in bit..end {
        let (byte, shift) = (b / 8, 7 - b % 8);
        let v = data.get_mut(byte)?;
        if set {
            *v |= 1 << shift;
        } else {
            *v &= !(1 << shift);
        }
    }
    Some(())
}
