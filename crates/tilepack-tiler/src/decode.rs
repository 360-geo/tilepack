//! Source image decoding into [`RgbSlab`]. Pure-Rust backends: zune-jpeg for
//! JPEG, the `png` crate for PNG.

use crate::TilerError;
use crate::slab::RgbSlab;

/// Decode a JPEG or PNG byte buffer into an 8-bit RGB slab.
pub fn decode_rgb(bytes: &[u8]) -> Result<RgbSlab, TilerError> {
    if bytes.len() >= 3 && bytes[0..3] == [0xFF, 0xD8, 0xFF] {
        decode_jpeg(bytes)
    } else if bytes.len() >= 8 && bytes[0..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        decode_png(bytes)
    } else {
        Err(TilerError::Io("source is neither JPEG nor PNG".into()))
    }
}

#[cfg(not(feature = "turbojpeg"))]
fn decode_jpeg(bytes: &[u8]) -> Result<RgbSlab, TilerError> {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(bytes, opts);
    let pixels = decoder.decode().map_err(|e| TilerError::Io(format!("jpeg decode: {e:?}")))?;
    let info = decoder.info().ok_or_else(|| TilerError::Io("jpeg info missing".into()))?;
    let (w, h) = (info.width as u32, info.height as u32);
    if pixels.len() != (w as usize) * (h as usize) * 3 {
        return Err(TilerError::Io("jpeg did not decode to RGB".into()));
    }
    Ok(RgbSlab::from_data(w, h, pixels))
}

#[cfg(feature = "turbojpeg")]
fn decode_jpeg(bytes: &[u8]) -> Result<RgbSlab, TilerError> {
    let image = turbojpeg::decompress(bytes, turbojpeg::PixelFormat::RGB).map_err(|e| TilerError::Io(format!("turbojpeg decode: {e}")))?;
    let (w, h) = (image.width as u32, image.height as u32);
    // Repack from the decoder pitch to a tight w*3 stride.
    let mut data = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for row in 0..image.height {
        let start = row * image.pitch;
        data.extend_from_slice(&image.pixels[start..start + (w as usize) * 3]);
    }
    Ok(RgbSlab::from_data(w, h, data))
}

fn decode_png(bytes: &[u8]) -> Result<RgbSlab, TilerError> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().map_err(|e| TilerError::Io(format!("png header: {e}")))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| TilerError::Io(format!("png frame: {e}")))?;
    let (w, h) = (info.width, info.height);
    buf.truncate(info.buffer_size());

    let rgb = match info.color_type {
        png::ColorType::Rgb => buf,
        png::ColorType::Rgba => {
            let mut out = Vec::with_capacity((w as usize) * (h as usize) * 3);
            for px in buf.chunks_exact(4) {
                out.extend_from_slice(&px[0..3]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity((w as usize) * (h as usize) * 3);
            for &g in &buf {
                out.extend_from_slice(&[g, g, g]);
            }
            out
        }
        other => return Err(TilerError::Io(format!("unsupported PNG color type {other:?}"))),
    };
    Ok(RgbSlab::from_data(w, h, rgb))
}
