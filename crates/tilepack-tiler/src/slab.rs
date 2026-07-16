//! Simple owned pixel buffers used across the converter. No `image` crate in
//! the public API — just tightly-packed rows.

/// A tightly packed 8-bit RGB image, row-major, 3 bytes per pixel.
#[derive(Debug, Clone)]
pub struct RgbSlab {
    pub w: u32,
    pub h: u32,
    /// `w * h * 3` bytes, row-major RGB.
    pub data: Vec<u8>,
}

impl RgbSlab {
    pub fn new(w: u32, h: u32) -> RgbSlab {
        RgbSlab {
            w,
            h,
            data: vec![0u8; (w as usize) * (h as usize) * 3],
        }
    }

    pub fn from_data(w: u32, h: u32, data: Vec<u8>) -> RgbSlab {
        assert_eq!(data.len(), (w as usize) * (h as usize) * 3, "RgbSlab data must be w*h*3");
        RgbSlab { w, h, data }
    }

    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y as usize) * (self.w as usize) + x as usize) * 3;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    #[inline]
    pub fn set_pixel(&mut self, x: u32, y: u32, rgb: [u8; 3]) {
        let i = ((y as usize) * (self.w as usize) + x as usize) * 3;
        self.data[i] = rgb[0];
        self.data[i + 1] = rgb[1];
        self.data[i + 2] = rgb[2];
    }
}

/// A tightly packed 16-bit single-channel raster (NIR / TIR / depth counts),
/// row-major, `0` conventionally nodata for raw-value groups.
#[derive(Debug, Clone)]
pub struct U16Slab {
    pub w: u32,
    pub h: u32,
    /// `w * h` counts, row-major.
    pub data: Vec<u16>,
}

impl U16Slab {
    pub fn new(w: u32, h: u32) -> U16Slab {
        U16Slab {
            w,
            h,
            data: vec![0u16; (w as usize) * (h as usize)],
        }
    }

    pub fn from_data(w: u32, h: u32, data: Vec<u16>) -> U16Slab {
        assert_eq!(data.len(), (w as usize) * (h as usize), "U16Slab data must be w*h");
        U16Slab { w, h, data }
    }
}
