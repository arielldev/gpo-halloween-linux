use std::io::Write;
use std::process::{Command, Stdio};

use parking_lot::Mutex;
use x11rb::protocol::xproto::{ConnectionExt, Keycode, KeyButMask, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT};
use x11rb::protocol::xtest::ConnectionExt as XTest;
use x11rb::CURRENT_TIME;

use super::x::display;
use crate::core::platform::Input;
use crate::core::types::{Key, MouseButton, PxPoint, PxRect};

const XK_BACKSPACE: u32 = 0xff08;
const XK_TAB: u32 = 0xff09;
const XK_RETURN: u32 = 0xff0d;
const XK_ESCAPE: u32 = 0xff1b;
const XK_DELETE: u32 = 0xffff;
const XK_SHIFT_L: u32 = 0xffe1;
const XK_CONTROL_L: u32 = 0xffe3;
const XK_ALT_L: u32 = 0xffe9;
const XK_F1: u32 = 0xffbe;
const XK_SPACE: u32 = 0x20;

pub fn char_keysym(c: char) -> Option<u32> {
    let cp = c as u32;
    match cp {
        0x20..=0x7e | 0xa0..=0xff => Some(cp),
        0x100..=0x10ffff => Some(0x0100_0000 | cp),
        _ => None,
    }
}

pub fn key_keysym(k: Key) -> Option<u32> {
    Some(match k {
        Key::Backspace => XK_BACKSPACE,
        Key::Delete => XK_DELETE,
        Key::Enter => XK_RETURN,
        Key::Escape => XK_ESCAPE,
        Key::Tab => XK_TAB,
        Key::Space => XK_SPACE,
        Key::Shift => XK_SHIFT_L,
        Key::Control => XK_CONTROL_L,
        Key::Alt => XK_ALT_L,
        Key::F(n) => XK_F1 + n.clamp(1, 24) as u32 - 1,
        Key::Char(c) => return char_keysym(c),
    })
}

struct Keymap {
    min: Keycode,
    per: usize,
    syms: Vec<u32>,
}

impl Keymap {
    fn load() -> Option<Keymap> {
        let d = display()?;
        let setup = x11rb::connection::Connection::setup(&d.conn);
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let r = d.conn.get_keyboard_mapping(min, max - min + 1).ok()?.reply().ok()?;
        Some(Keymap { min, per: r.keysyms_per_keycode as usize, syms: r.keysyms })
    }

    fn find(&self, sym: u32) -> Option<(Keycode, bool)> {
        for (i, chunk) in self.syms.chunks(self.per.max(1)).enumerate() {
            if chunk.first() == Some(&sym) {
                return Some((self.min + i as u8, false));
            }
            if chunk.get(1) == Some(&sym) {
                return Some((self.min + i as u8, true));
            }
        }
        None
    }
}

pub struct XTestInput {
    keymap: Mutex<Option<Keymap>>,
}

fn raw_pointer() {
    let Some(d) = display() else { return };
    if d.conn.change_pointer_control(1, 1, 0, true, true).is_ok() {
        d.flush();
    }
}

impl XTestInput {
    fn step_to(&self, p: PxPoint) {
        let cur = self.cursor();
        let (dx, dy) = (p.x - cur.x, p.y - cur.y);
        if dx != 0 || dy != 0 {
            self.fake(MOTION_NOTIFY_EVENT, 1, dx as i16, dy as i16);
        }
        self.fake(MOTION_NOTIFY_EVENT, 1, 0, 1);
        self.fake(MOTION_NOTIFY_EVENT, 1, 0, -1);
        if let Some(d) = display() {
            d.flush();
        }
    }
}

impl Default for XTestInput {
    fn default() -> Self {
        Self::new()
    }
}

impl XTestInput {
    pub fn new() -> Self {
        raw_pointer();
        Self { keymap: Mutex::new(Keymap::load()) }
    }

    fn code(&self, sym: u32) -> Option<(Keycode, bool)> {
        let mut km = self.keymap.lock();
        if km.is_none() {
            *km = Keymap::load();
        }
        km.as_ref()?.find(sym)
    }

    fn fake(&self, ty: u8, detail: u8, x: i16, y: i16) {
        let Some(d) = display() else { return };
        let _ = d.conn.xtest_fake_input(ty, detail, CURRENT_TIME, d.root, x, y, 0);
    }

    fn press_sym(&self, sym: u32, down: bool) {
        let Some((code, shifted)) = self.code(sym) else { return };
        let shift = self.code(XK_SHIFT_L).map(|c| c.0);
        if shifted && down {
            if let Some(s) = shift {
                self.fake(KEY_PRESS_EVENT, s, 0, 0);
            }
        }
        self.fake(if down { KEY_PRESS_EVENT } else { KEY_RELEASE_EVENT }, code, 0, 0);
        if shifted && !down {
            if let Some(s) = shift {
                self.fake(KEY_RELEASE_EVENT, s, 0, 0);
            }
        }
        if let Some(d) = display() {
            d.flush();
        }
    }
}

impl Input for XTestInput {
    fn move_to(&self, p: PxPoint) {
        self.step_to(p);
        std::thread::sleep(std::time::Duration::from_millis(30));
        if self.cursor() != p {
            self.fake(BUTTON_RELEASE_EVENT, 3, 0, 0);
            if let Some(d) = display() {
                d.flush();
            }
            std::thread::sleep(std::time::Duration::from_millis(60));
            self.step_to(p);
        }
    }

    fn move_rel(&self, dx: i32, dy: i32) {
        self.fake(MOTION_NOTIFY_EVENT, 1, dx as i16, dy as i16);
        if let Some(d) = display() {
            d.flush();
        }
    }

    fn button(&self, b: MouseButton, down: bool) {
        let n = match b {
            MouseButton::Left => 1,
            MouseButton::Right => 3,
        };
        self.fake(if down { BUTTON_PRESS_EVENT } else { BUTTON_RELEASE_EVENT }, n, 0, 0);
        if let Some(d) = display() {
            d.flush();
        }
    }

    fn key(&self, k: Key, down: bool) {
        if let Some(sym) = key_keysym(k) {
            self.press_sym(sym, down);
        }
    }

    fn wheel(&self, delta: i32) {
        let button = if delta > 0 { 4 } else { 5 };
        let steps = (delta.abs() / 120).max(1);
        for _ in 0..steps {
            self.fake(BUTTON_PRESS_EVENT, button, 0, 0);
            self.fake(BUTTON_RELEASE_EVENT, button, 0, 0);
        }
        if let Some(d) = display() {
            d.flush();
        }
    }

    fn type_text(&self, s: &str) {
        for c in s.chars() {
            if let Some(sym) = char_keysym(c) {
                self.press_sym(sym, true);
                self.press_sym(sym, false);
            }
        }
    }

    fn cursor(&self) -> PxPoint {
        display()
            .and_then(|d| d.conn.query_pointer(d.root).ok()?.reply().ok())
            .map(|r| PxPoint { x: r.root_x as i32, y: r.root_y as i32 })
            .unwrap_or_default()
    }

    fn screen(&self) -> PxRect {
        display().map(|d| PxRect { x: 0, y: 0, w: d.width as i32, h: d.height as i32 }).unwrap_or_default()
    }

    fn key_down(&self, k: Key) -> bool {
        let Some(sym) = key_keysym(k) else { return false };
        let Some((code, _)) = self.code(sym) else { return false };
        display()
            .and_then(|d| d.conn.query_keymap().ok()?.reply().ok())
            .map(|r| r.keys[(code / 8) as usize] & (1 << (code % 8)) != 0)
            .unwrap_or(false)
    }

    fn button_down(&self, b: MouseButton) -> bool {
        let mask = match b {
            MouseButton::Left => KeyButMask::BUTTON1,
            MouseButton::Right => KeyButMask::BUTTON3,
        };
        display()
            .and_then(|d| d.conn.query_pointer(d.root).ok()?.reply().ok())
            .map(|r| r.mask.contains(mask))
            .unwrap_or(false)
    }

    fn set_clipboard(&self, text: &str) -> bool {
        let Ok(mut child) = Command::new("xclip").args(["-selection", "clipboard"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
            return false;
        };
        let wrote = child.stdin.take().map(|mut s| s.write_all(text.as_bytes()).is_ok()).unwrap_or(false);
        wrote && child.wait().map(|s| s.success()).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_keys_to_keysyms() {
        assert_eq!(key_keysym(Key::Char('e')), Some(0x65));
        assert_eq!(key_keysym(Key::Char('E')), Some(0x45));
        assert_eq!(key_keysym(Key::Enter), Some(0xff0d));
        assert_eq!(key_keysym(Key::F(11)), Some(0xffc8));
        assert_eq!(char_keysym('é'), Some(0xe9));
        assert_eq!(char_keysym('א'), Some(0x0100_05d0));
        assert_eq!(char_keysym('\u{7}'), None);
    }
}
