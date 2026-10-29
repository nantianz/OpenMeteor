use image::{ImageBuffer, ImageFormat, RgbaImage};

#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,

    pub rgba: Vec<u8>,
}

pub fn decode_image(bytes: &[u8]) -> Result<DecodedImage, String> {
    let rgba = if looks_like_tga(bytes) {
        decode_tga(bytes)?
    } else if looks_like_bmp(bytes) {
        decode_bmp(bytes)?
    } else {
        let image = image::load_from_memory(bytes).map_err(|e| format!("image decode: {e}"))?;
        image.to_rgba8()
    };
    let (width, height) = rgba.dimensions();
    let flipped = image::imageops::flip_vertical(&rgba);
    Ok(DecodedImage {
        width,
        height,
        rgba: flipped.into_raw(),
    })
}

pub fn probe_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    if looks_like_tga(bytes) {
        return Some(ImageFormat::Tga);
    }
    if looks_like_bmp(bytes) {
        return Some(ImageFormat::Bmp);
    }
    image::guess_format(bytes).ok()
}

fn looks_like_bmp(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == b'B' && bytes[1] == b'M'
}

fn looks_like_tga(bytes: &[u8]) -> bool {
    bytes.len() >= 18 && bytes[1] == 0 && (bytes[2] == 2 || bytes[2] == 10)
}

fn decode_tga(bytes: &[u8]) -> Result<RgbaImage, String> {
    if bytes.len() < 18 {
        return Err("TGA too short".into());
    }
    let id_len = bytes[0] as usize;
    let color_map_type = bytes[1];
    let image_type = bytes[2];
    let width = u16::from_le_bytes([bytes[12], bytes[13]]) as u32;
    let height = u16::from_le_bytes([bytes[14], bytes[15]]) as u32;
    let bpp = bytes[16];
    let descriptor = bytes[17];
    let top_left = (descriptor & 0x20) != 0;

    if color_map_type != 0 || (image_type != 2 && image_type != 10) {
        return Err(format!(
            "unsupported TGA: imageType={image_type} colorMapType={color_map_type}"
        ));
    }
    if bpp != 24 && bpp != 32 {
        return Err(format!("unsupported TGA bpp={bpp}"));
    }
    if width == 0 || height == 0 {
        return Err(format!("invalid TGA dimensions {width}x{height}"));
    }

    let bytes_pp = (bpp / 8) as usize;
    let mut off = 18 + id_len;
    let total = (width as u64 * height as u64) as usize;
    let mut pixels = vec![0u8; total * 4];
    let mut out = 0usize;

    let mut put_bgra = |src: &[u8], i: usize| -> Result<(), String> {
        if i + bytes_pp > src.len() || out + 4 > pixels.len() {
            return Err("TGA pixel overrun".into());
        }
        pixels[out] = src[i + 2];
        pixels[out + 1] = src[i + 1];
        pixels[out + 2] = src[i];
        pixels[out + 3] = if bytes_pp == 4 { src[i + 3] } else { 255 };
        out += 4;
        Ok(())
    };

    if image_type == 2 {
        for _ in 0..total {
            put_bgra(bytes, off)?;
            off += bytes_pp;
        }
    } else {
        let mut p = 0usize;
        while p < total {
            if off >= bytes.len() {
                return Err("TGA RLE overrun".into());
            }
            let header = bytes[off];
            off += 1;
            let count = ((header & 0x7f) as usize) + 1;
            if header & 0x80 != 0 {
                let i = off;
                off += bytes_pp;
                for _ in 0..count {
                    put_bgra(bytes, i)?;
                }
            } else {
                for _ in 0..count {
                    put_bgra(bytes, off)?;
                    off += bytes_pp;
                }
            }
            p += count;
        }
    }

    if !top_left {
        let row_bytes = (width as usize) * 4;
        let mut flipped = vec![0u8; pixels.len()];
        for y in 0..height as usize {
            let src_row = (height as usize - 1 - y) * row_bytes;
            let dst_row = y * row_bytes;
            flipped[dst_row..dst_row + row_bytes]
                .copy_from_slice(&pixels[src_row..src_row + row_bytes]);
        }
        pixels = flipped;
    }

    ImageBuffer::from_raw(width, height, pixels)
        .ok_or_else(|| "TGA pixel buffer size mismatch".into())
}

fn decode_bmp(bytes: &[u8]) -> Result<RgbaImage, String> {
    if bytes.len() < 54 || bytes[0] != b'B' || bytes[1] != b'M' {
        return Err("invalid BMP signature".into());
    }
    let pixel_offset = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]) as usize;
    let dib_size = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]) as usize;
    if dib_size < 40 {
        return Err(format!("unsupported BMP DIB header size {dib_size}"));
    }
    let width = i32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
    let signed_height = i32::from_le_bytes([bytes[22], bytes[23], bytes[24], bytes[25]]);
    let height = signed_height.unsigned_abs();
    let bpp = u16::from_le_bytes([bytes[28], bytes[29]]);
    let compression = u32::from_le_bytes([bytes[30], bytes[31], bytes[32], bytes[33]]);
    if width <= 0 || height == 0 {
        return Err(format!("invalid BMP dimensions {width}x{signed_height}"));
    }
    if bpp != 8 && bpp != 24 && bpp != 32 {
        return Err(format!("unsupported BMP bpp={bpp}"));
    }
    if compression != 0 {
        return Err(format!("unsupported BMP compression={compression}"));
    }

    let width = width as u32;
    let bottom_up = signed_height > 0;
    let row_stride = (width as usize * bpp as usize).div_ceil(32) * 4;
    let bytes_pp = (bpp / 8) as usize;
    let mut palette: Vec<[u8; 4]> = Vec::new();
    if bpp == 8 {
        let declared_colors = u32::from_le_bytes([bytes[46], bytes[47], bytes[48], bytes[49]]);
        let colors_used = if declared_colors == 0 {
            256
        } else {
            declared_colors as usize
        };
        let palette_offset = 14 + dib_size;
        for i in 0..colors_used {
            let at = palette_offset + i * 4;
            if at + 3 >= bytes.len() {
                break;
            }
            palette.push([bytes[at + 2], bytes[at + 1], bytes[at], 255]);
        }
    }

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for y in 0..height as usize {
        let source_y = if bottom_up {
            height as usize - 1 - y
        } else {
            y
        };
        let row = pixel_offset + source_y * row_stride;
        for x in 0..width as usize {
            let out = (y * width as usize + x) * 4;
            if bpp == 8 {
                if row + x >= bytes.len() {
                    continue;
                }
                let idx = bytes[row + x] as usize;
                let color = palette.get(idx).copied().unwrap_or([0, 0, 0, 255]);
                pixels[out..out + 4].copy_from_slice(&color);
            } else {
                let at = row + x * bytes_pp;
                if at + bytes_pp > bytes.len() {
                    continue;
                }
                pixels[out] = bytes[at + 2];
                pixels[out + 1] = bytes[at + 1];
                pixels[out + 2] = bytes[at];
                pixels[out + 3] = if bytes_pp == 4 { bytes[at + 3] } else { 255 };
            }
        }
    }

    ImageBuffer::from_raw(width, height, pixels)
        .ok_or_else(|| "BMP pixel buffer size mismatch".into())
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn flip_vertical_swaps_rows() {
        let mut img = image::RgbaImage::new(1, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        let mut cursor = std::io::Cursor::new(Vec::new());
        img.write_to(&mut cursor, image::ImageFormat::Png)
            .expect("encode png");
        let decoded = decode_image(&cursor.into_inner()).expect("decode");
        assert_eq!((decoded.width, decoded.height), (1, 2));

        assert_eq!(&decoded.rgba[0..4], &[0, 0, 255, 255]);
        assert_eq!(&decoded.rgba[4..8], &[255, 0, 0, 255]);
    }

    #[test]
    fn bmp_declared_palette_does_not_consume_pixel_bytes_as_colours() {
        let mut bytes = vec![0u8; 62];
        bytes[..2].copy_from_slice(b"BM");
        bytes[10..14].copy_from_slice(&58u32.to_le_bytes());
        bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
        bytes[18..22].copy_from_slice(&1i32.to_le_bytes());
        bytes[22..26].copy_from_slice(&1i32.to_le_bytes());
        bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
        bytes[28..30].copy_from_slice(&8u16.to_le_bytes());
        bytes[46..50].copy_from_slice(&1u32.to_le_bytes());
        bytes[54..58].copy_from_slice(&[0, 0, 255, 0]);

        bytes[58..62].copy_from_slice(&[1, 22, 33, 0]);
        assert_eq!(decode_image(&bytes).unwrap().rgba, [0, 0, 0, 255]);
        bytes[58] = 0;
        assert_eq!(decode_image(&bytes).unwrap().rgba, [255, 0, 0, 255]);
    }

    #[test]
    fn probe_rejects_garbage() {
        assert!(probe_image_format(b"not an image").is_none());
    }

    #[test]
    fn origin_res_ptexture_tga_decodes() {
        use std::io::Read;
        let pak_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/ptexture.pak");
        if !pak_path.is_file() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let file = std::fs::File::open(pak_path).expect("open ptexture.pak");
        let mut archive = zip::ZipArchive::new(file).expect("open zip");
        let mut entry = archive.by_name("ptexture/nw001b01.tga").expect("entry");
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("read entry");
        let decoded = decode_image(&bytes).expect("decode nw001b01.tga");
        assert_eq!((decoded.width, decoded.height), (256, 256));
        assert_eq!(decoded.rgba.len(), 256 * 256 * 4);
    }
}
