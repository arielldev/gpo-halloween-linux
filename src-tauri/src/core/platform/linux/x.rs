use std::sync::OnceLock;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt, Window};
use x11rb::rust_connection::RustConnection;

pub struct Display {
    pub conn: RustConnection,
    pub root: Window,
    pub width: u16,
    pub height: u16,
    pub atoms: Atoms,
}

pub struct Atoms {
    pub net_client_list: Atom,
    pub net_active_window: Atom,
    pub net_wm_name: Atom,
    pub net_wm_pid: Atom,
    pub net_wm_state: Atom,
    pub net_wm_state_maximized_vert: Atom,
    pub net_wm_state_maximized_horz: Atom,
    pub net_wm_state_fullscreen: Atom,
    pub net_wm_state_hidden: Atom,
    pub utf8_string: Atom,
}

fn intern(conn: &RustConnection, name: &str) -> Atom {
    conn.intern_atom(false, name.as_bytes())
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| r.atom)
        .unwrap_or(AtomEnum::NONE.into())
}

fn open() -> Option<Display> {
    let (conn, screen) = x11rb::connect(None).ok()?;
    let s = conn.setup().roots.get(screen)?.clone();
    let atoms = Atoms {
        net_client_list: intern(&conn, "_NET_CLIENT_LIST"),
        net_active_window: intern(&conn, "_NET_ACTIVE_WINDOW"),
        net_wm_name: intern(&conn, "_NET_WM_NAME"),
        net_wm_pid: intern(&conn, "_NET_WM_PID"),
        net_wm_state: intern(&conn, "_NET_WM_STATE"),
        net_wm_state_maximized_vert: intern(&conn, "_NET_WM_STATE_MAXIMIZED_VERT"),
        net_wm_state_maximized_horz: intern(&conn, "_NET_WM_STATE_MAXIMIZED_HORZ"),
        net_wm_state_fullscreen: intern(&conn, "_NET_WM_STATE_FULLSCREEN"),
        net_wm_state_hidden: intern(&conn, "_NET_WM_STATE_HIDDEN"),
        utf8_string: intern(&conn, "UTF8_STRING"),
    };
    Some(Display { conn, root: s.root, width: s.width_in_pixels, height: s.height_in_pixels, atoms })
}

pub fn display() -> Option<&'static Display> {
    static D: OnceLock<Option<Display>> = OnceLock::new();
    let d = D.get_or_init(|| {
        let d = open();
        if d.is_none() {
            tracing::warn!("no X display reachable (is DISPLAY set?)");
        }
        d
    });
    d.as_ref()
}

impl Display {
    pub fn u32_list(&self, win: Window, prop: Atom, ty: impl Into<Atom>) -> Vec<u32> {
        self.conn
            .get_property(false, win, prop, ty, 0, 4096)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect()))
            .unwrap_or_default()
    }

    pub fn title(&self, win: Window) -> String {
        let read = |prop: Atom, ty: Atom| {
            self.conn
                .get_property(false, win, prop, ty, 0, 1024)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| String::from_utf8_lossy(&r.value).into_owned())
                .unwrap_or_default()
        };
        let name = read(self.atoms.net_wm_name, self.atoms.utf8_string);
        if !name.is_empty() {
            return name;
        }
        read(AtomEnum::WM_NAME.into(), AtomEnum::STRING.into())
    }

    pub fn clients(&self) -> Vec<Window> {
        self.u32_list(self.root, self.atoms.net_client_list, AtomEnum::WINDOW)
    }

    pub fn active(&self) -> Option<Window> {
        self.u32_list(self.root, self.atoms.net_active_window, AtomEnum::WINDOW).first().copied().filter(|w| *w != 0)
    }

    pub fn pid_of(&self, win: Window) -> Option<u32> {
        self.u32_list(win, self.atoms.net_wm_pid, AtomEnum::CARDINAL).first().copied()
    }

    pub fn active_is_ours(&self) -> bool {
        self.active().and_then(|w| self.pid_of(w)).is_some_and(|p| p == std::process::id())
    }

    pub fn input_focus(&self) -> Option<Window> {
        self.conn.get_input_focus().ok().and_then(|c| c.reply().ok()).map(|r| r.focus)
    }

    pub fn flush(&self) {
        let _ = self.conn.flush();
    }
}
