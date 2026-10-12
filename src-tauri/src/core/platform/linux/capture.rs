use x11rb::protocol::xproto::{ConnectionExt, ImageFormat};

use super::x::display;
use crate::core::platform::{Capture, PlatformError, Result};
use crate::core::types::{Frame, PxRect};

#[derive(Default)]
pub struct X11Capture;

impl X11Capture {
    pub fn new() -> Self {
        Self
    }
}

pub fn bgrx_to_rgba(mut buf: Vec<u8>) -> Vec<u8> {
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    buf
}

impl Capture for X11Capture {
    fn grab(&self, rect: PxRect) -> Result<Frame> {
        if rect.is_empty() {
            return Err(PlatformError::Capture("empty rect".into()));
        }
        let d = display().ok_or_else(|| PlatformError::Capture("no X display".into()))?;
        let x = rect.x.clamp(0, d.width as i32 - 1);
        let y = rect.y.clamp(0, d.height as i32 - 1);
        let w = rect.w.min(d.width as i32 - x).max(1);
        let h = rect.h.min(d.height as i32 - y).max(1);
        let reply = d
            .conn
            .get_image(ImageFormat::Z_PIXMAP, d.root, x as i16, y as i16, w as u16, h as u16, u32::MAX)
            .map_err(|e| PlatformError::Capture(e.to_string()))?
            .reply()
            .map_err(|e| PlatformError::Capture(e.to_string()))?;
        if reply.depth < 24 || reply.data.len() < (w * h * 4) as usize {
            return Err(PlatformError::Capture(format!("unsupported X image depth {}", reply.depth)));
        }
        let mut data = reply.data;
        data.truncate((w * h * 4) as usize);
        let rgba = bgrx_to_rgba(data);
        if rgba.chunks_exact(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0) {
            return Err(PlatformError::Capture("screen capture came back all black".into()));
        }
        Ok(Frame::new(w as usize, h as usize, rgba))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_bgrx_pixels() {
        assert_eq!(bgrx_to_rgba(vec![1, 2, 3, 0, 9, 8, 7, 0]), vec![3, 2, 1, 255, 7, 8, 9, 255]);
    }
}
