//! Bring gerdr's window to the front on the host compositor, for a click on
//! one of its desktop notifications. winit's `focus_window` does nothing on
//! Wayland, so gerdr asks the host itself over xdg-activation, on winit's own
//! connection (the surface to activate lives there), with a queue of its own.
//!
//! The host only raises a window for a token that comes from a user action:
//! the notification daemon's, sent with the click. Many daemons send none
//! (Quickshell's never does); then gerdr asks niri over its IPC, when it
//! runs on niri, or else makes a token of its own, which most hosts take as
//! a request for attention only.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    time::Duration,
};

use smithay::reexports::winit::{
    raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle},
    window::Window,
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    backend::{Backend, ObjectId},
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry::WlRegistry, wl_surface::WlSurface},
};
use wayland_protocols::xdg::activation::v1::client::{
    xdg_activation_token_v1::{self, XdgActivationTokenV1},
    xdg_activation_v1::XdgActivationV1,
};

/// What the queue's events land in: the token made for a request without one.
#[derive(Default)]
struct Events {
    token: Option<String>,
}

/// The host's xdg-activation, for gerdr's window.
pub struct Activation {
    conn: Connection,
    queue: EventQueue<Events>,
    activation: XdgActivationV1,
    surface: WlSurface,
}

impl Activation {
    /// Bind xdg-activation on the connection `window` is on. `None` off
    /// Wayland or on a host without it.
    pub fn new(window: &Window) -> Option<Self> {
        let RawDisplayHandle::Wayland(display) = window.display_handle().ok()?.as_raw() else {
            return None;
        };
        let RawWindowHandle::Wayland(handle) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        // Safety: winit's display and surface outlive gerdr's window, which
        // lives as long as gerdr.
        let conn = Connection::from_backend(unsafe {
            Backend::from_foreign_display(display.display.as_ptr().cast())
        });
        let id =
            unsafe { ObjectId::from_ptr(WlSurface::interface(), handle.surface.as_ptr().cast()) }
                .ok()?;
        let surface = WlSurface::from_id(&conn, id).ok()?;
        let (globals, queue) = registry_queue_init::<Events>(&conn)
            .map_err(|err| tracing::warn!("window activation: {err}"))
            .ok()?;
        let activation = globals
            .bind::<XdgActivationV1, _, _>(&queue.handle(), 1..=1, ())
            .map_err(|err| tracing::warn!("window activation: {err}"))
            .ok()?;
        Some(Self {
            conn,
            queue,
            activation,
            surface,
        })
    }

    /// Ask the host to show and focus gerdr's window, with `token` from the
    /// notification daemon if it sent one.
    pub fn activate(&mut self, token: Option<&str>) {
        let token = match token {
            Some(token) => token.to_owned(),
            None => match self.own_token() {
                Some(token) => token,
                None => return,
            },
        };
        self.activation.activate(token, &self.surface);
        if let Err(err) = self.conn.flush() {
            tracing::warn!("window activation: {err}");
        }
    }

    /// A token made by gerdr itself, waited for.
    fn own_token(&mut self) -> Option<String> {
        let request = self
            .activation
            .get_activation_token(&self.queue.handle(), ());
        request.set_app_id("gerdr".to_owned());
        request.set_surface(&self.surface);
        request.commit();
        let mut events = Events::default();
        let got = self.queue.roundtrip(&mut events);
        request.destroy();
        got.map_err(|err| tracing::warn!("window activation: {err}"))
            .ok()?;
        events.token
    }
}

/// Focus gerdr's window, the one of gerdr's process, through niri's IPC
/// (`$NIRI_SOCKET`). Whether niri did; `false` off niri.
pub fn niri_focus() -> bool {
    let Some(socket) = std::env::var_os("NIRI_SOCKET") else {
        return false;
    };
    let Some(windows) = niri_request(&socket, &serde_json::json!("Windows")) else {
        return false;
    };
    let pid = u64::from(std::process::id());
    let id = windows["Ok"]["Windows"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|w| w["pid"].as_u64() == Some(pid))
        .and_then(|w| w["id"].as_u64());
    let Some(id) = id else {
        tracing::warn!("window activation: niri has no window of gerdr's");
        return false;
    };
    let focus = serde_json::json!({ "Action": { "FocusWindow": { "id": id } } });
    niri_request(&socket, &focus).is_some_and(|reply| reply.get("Ok").is_some())
}

/// Send niri one IPC request and read its reply.
fn niri_request(
    socket: &std::ffi::OsStr,
    request: &serde_json::Value,
) -> Option<serde_json::Value> {
    let result = (|| -> std::io::Result<serde_json::Value> {
        let mut stream = UnixStream::connect(socket)?;
        stream.set_read_timeout(Some(Duration::from_secs(1)))?;
        stream.set_write_timeout(Some(Duration::from_secs(1)))?;
        writeln!(stream, "{request}")?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    })();
    result
        .map_err(|err| tracing::warn!("window activation: niri: {err}"))
        .ok()
}

impl Dispatch<WlRegistry, GlobalListContents> for Events {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgActivationV1, ()> for Events {
    fn event(
        _: &mut Self,
        _: &XdgActivationV1,
        _: <XdgActivationV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgActivationTokenV1, ()> for Events {
    fn event(
        state: &mut Self,
        _: &XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            state.token = Some(token);
        }
    }
}
