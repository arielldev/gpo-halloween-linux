use std::ffi::{c_char, c_int, c_void, CStr};
use std::hash::{Hash, Hasher};
use std::io::{Cursor, Write};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use crate::core::platform::{Ocr, PlatformError, Result};
use crate::core::types::Frame;

const PSM_SINGLE_BLOCK: c_int = 6;

type Create = unsafe extern "C" fn() -> *mut c_void;
type Init3 = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
type SetPsm = unsafe extern "C" fn(*mut c_void, c_int);
type SetImage = unsafe extern "C" fn(*mut c_void, *const u8, c_int, c_int, c_int, c_int);
type GetText = unsafe extern "C" fn(*mut c_void) -> *mut c_char;
type DeleteText = unsafe extern "C" fn(*const c_char);
type Clear = unsafe extern "C" fn(*mut c_void);

struct Engine {
    _lib: libloading::Library,
    api: *mut c_void,
    set_image: SetImage,
    get_text: GetText,
    delete_text: DeleteText,
    clear: Clear,
}

unsafe impl Send for Engine {}

impl Engine {
    fn load() -> Option<Engine> {
        let lib = ["libtesseract.so.5", "libtesseract.so"].iter().find_map(|n| unsafe { libloading::Library::new(n).ok() })?;
        unsafe {
            let create = *lib.get::<Create>(b"TessBaseAPICreate\0").ok()?;
            let init = *lib.get::<Init3>(b"TessBaseAPIInit3\0").ok()?;
            let set_psm = *lib.get::<SetPsm>(b"TessBaseAPISetPageSegMode\0").ok()?;
            let set_image = *lib.get::<SetImage>(b"TessBaseAPISetImage\0").ok()?;
            let get_text = *lib.get::<GetText>(b"TessBaseAPIGetUTF8Text\0").ok()?;
            let delete_text = *lib.get::<DeleteText>(b"TessDeleteText\0").ok()?;
            let clear = *lib.get::<Clear>(b"TessBaseAPIClear\0").ok()?;
            let api = create();
            if api.is_null() || init(api, std::ptr::null(), c"eng".as_ptr()) != 0 {
                return None;
            }
            set_psm(api, PSM_SINGLE_BLOCK);
            Some(Engine { _lib: lib, api, set_image, get_text, delete_text, clear })
        }
    }

    fn read(&mut self, frame: &Frame) -> Option<String> {
        unsafe {
            (self.set_image)(self.api, frame.rgba.as_ptr(), frame.w as c_int, frame.h as c_int, 4, (frame.w * 4) as c_int);
            let raw = (self.get_text)(self.api);
            if raw.is_null() {
                (self.clear)(self.api);
                return None;
            }
            let text = CStr::from_ptr(raw).to_string_lossy().trim().to_string();
            (self.delete_text)(raw);
            (self.clear)(self.api);
            Some(text)
        }
    }
}

#[derive(Default)]
pub struct TesseractOcr {
    present: OnceLock<bool>,
    engine: OnceLock<Option<Mutex<Engine>>>,
    last: Mutex<Option<(u64, String)>>,
}

impl TesseractOcr {
    pub fn new() -> Self {
        Self::default()
    }

    fn engine(&self) -> Option<&Mutex<Engine>> {
        self.engine
            .get_or_init(|| {
                let e = Engine::load();
                if e.is_none() {
                    tracing::info!("libtesseract not loadable; reading through the tesseract command instead");
                }
                e.map(Mutex::new)
            })
            .as_ref()
    }
}

pub fn frame_key(frame: &Frame) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    frame.w.hash(&mut h);
    frame.h.hash(&mut h);
    frame.rgba.hash(&mut h);
    h.finish()
}

fn png(frame: &Frame) -> Result<Vec<u8>> {
    let img = image::RgbaImage::from_raw(frame.w as u32, frame.h as u32, frame.rgba.clone()).ok_or_else(|| PlatformError::Ocr("bad frame size".into()))?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| PlatformError::Ocr(e.to_string()))?;
    Ok(out.into_inner())
}

fn read_cli(frame: &Frame) -> Result<String> {
    let bytes = png(frame)?;
    let mut child = Command::new("tesseract")
        .args(["stdin", "stdout", "--psm", "6", "-l", "eng"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| PlatformError::Ocr(e.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&bytes).map_err(|e| PlatformError::Ocr(e.to_string()))?;
    }
    let out = child.wait_with_output().map_err(|e| PlatformError::Ocr(e.to_string()))?;
    if !out.status.success() {
        return Err(PlatformError::Ocr("tesseract failed".into()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

impl Ocr for TesseractOcr {
    fn available(&self) -> bool {
        *self.present.get_or_init(|| {
            if self.engine().is_some() {
                return true;
            }
            let ok = Command::new("tesseract").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
            if !ok {
                tracing::warn!("tesseract is not installed; the server timer cannot be read (backup clock only)");
            }
            ok
        })
    }

    fn read(&self, frame: &Frame) -> Result<String> {
        if !self.available() {
            return Err(PlatformError::OcrUnavailable);
        }
        if frame.w < 8 || frame.h < 8 || frame.rgba.len() < frame.w * frame.h * 4 {
            return Ok(String::new());
        }
        let key = frame_key(frame);
        if let Some((k, text)) = self.last.lock().ok().and_then(|l| l.clone()) {
            if k == key {
                return Ok(text);
            }
        }
        let text = match self.engine() {
            Some(e) => e.lock().ok().and_then(|mut e| e.read(frame)).ok_or_else(|| PlatformError::Ocr("tesseract failed".into()))?,
            None => read_cli(frame)?,
        };
        if let Ok(mut l) = self.last.lock() {
            *l = Some((key, text.clone()));
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_frames_share_a_key() {
        let a = Frame::new(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255]);
        let b = Frame::new(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255]);
        let c = Frame::new(2, 1, vec![1, 2, 3, 255, 4, 5, 7, 255]);
        assert_eq!(frame_key(&a), frame_key(&b));
        assert_ne!(frame_key(&a), frame_key(&c));
    }
}
