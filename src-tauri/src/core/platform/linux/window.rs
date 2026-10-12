use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt, EventMask, InputFocus, MapState, Window};
use x11rb::CURRENT_TIME;

use super::x::display;
use crate::core::platform::GameWindow;
use crate::core::types::{PxPoint, PxRect, WindowFix, WindowInfo};

pub struct X11Window {
    needles: Vec<String>,
}

impl X11Window {
    pub fn new(needles: &[&str]) -> Self {
        Self { needles: needles.iter().map(|s| s.to_ascii_lowercase()).collect() }
    }

    fn handle(&self) -> Option<Window> {
        let d = display()?;
        d.clients().into_iter().find(|w| {
            let t = d.title(*w).to_ascii_lowercase();
            self.needles.iter().any(|n| t.contains(n.as_str())) && !t.contains("macro")
        })
    }

    fn rect(&self, w: Window) -> Option<PxRect> {
        let d = display()?;
        let g = d.conn.get_geometry(w).ok()?.reply().ok()?;
        let t = d.conn.translate_coordinates(w, d.root, 0, 0).ok()?.reply().ok()?;
        Some(PxRect { x: t.dst_x as i32, y: t.dst_y as i32, w: g.width as i32, h: g.height as i32 })
    }

    fn states(&self, w: Window) -> Vec<u32> {
        display().map(|d| d.u32_list(w, d.atoms.net_wm_state, x11rb::protocol::xproto::AtomEnum::ATOM)).unwrap_or_default()
    }

    fn wm_message(&self, w: Window, ty: u32, data: [u32; 5]) {
        let Some(d) = display() else { return };
        let ev = ClientMessageEvent::new(32, w, ty, data);
        let _ = d.conn.send_event(false, d.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev);
        d.flush();
    }
}

impl GameWindow for X11Window {
    fn find(&self) -> Option<WindowInfo> {
        let d = display()?;
        let w = self.handle()?;
        let attrs = d.conn.get_window_attributes(w).ok()?.reply().ok()?;
        let hidden = self.states(w).contains(&d.atoms.net_wm_state_hidden);
        let visible = attrs.map_state == MapState::VIEWABLE && !hidden;
        let client = self.rect(w)?;
        if client.is_empty() && visible {
            return None;
        }
        Some(WindowInfo { client, is_foreground: d.active() == Some(w) || d.active_is_ours(), visible, dpi: 96 })
    }

    fn focus(&self) -> bool {
        let Some(d) = display() else { return false };
        let Some(w) = self.handle() else { return false };
        if d.active() == Some(w) {
            return true;
        }
        let _ = d.conn.map_window(w);
        self.wm_message(w, d.atoms.net_active_window, [2, CURRENT_TIME, 0, 0, 0]);
        let _ = d.conn.set_input_focus(InputFocus::PARENT, w, CURRENT_TIME);
        d.flush();
        for _ in 0..8 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if d.active() == Some(w) || d.input_focus() == Some(w) {
                return true;
            }
        }
        false
    }

    fn game_foreground(&self) -> bool {
        match (display(), self.handle()) {
            (Some(d), Some(w)) => d.active() == Some(w),
            _ => false,
        }
    }

    fn owns_point(&self, p: PxPoint) -> bool {
        self.handle().and_then(|w| self.rect(w)).map(|r| r.contains(p)).unwrap_or(false)
    }

    fn normalize(&self) -> WindowFix {
        let Some(d) = display() else { return WindowFix::None };
        let Some(w) = self.handle() else { return WindowFix::None };
        let st = self.states(w);
        if st.contains(&d.atoms.net_wm_state_fullscreen) {
            return WindowFix::Fullscreen;
        }
        if st.contains(&d.atoms.net_wm_state_maximized_vert) && st.contains(&d.atoms.net_wm_state_maximized_horz) {
            return WindowFix::None;
        }
        self.wm_message(w, d.atoms.net_wm_state, [1, d.atoms.net_wm_state_maximized_vert, d.atoms.net_wm_state_maximized_horz, 1, 0]);
        WindowFix::Maximized
    }
}
