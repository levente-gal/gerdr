//! Whether the user is away from the machine: no key or pointer on the host
//! for a while, as the host compositor tells over ext-idle-notify. It
//! decides what reaches the phone (see `web_push`) and whether an agent in
//! sight counts as seen.
//!
//! It listens on winit's own connection, with a queue of its own (as
//! `activate` does): winit reads the socket, and the queue's events wait
//! there until `away` looks.

use std::time::Duration;

use smithay::reexports::winit::{
    raw_window_handle::{HasDisplayHandle, RawDisplayHandle},
    window::Window,
};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle,
    backend::Backend,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

/// No input on the host for this long is away.
pub const AWAY: Duration = Duration::from_secs(120);

#[derive(Default)]
struct Idle(bool);

/// The host's word on whether the user is away.
pub struct HostIdle {
    queue: EventQueue<Idle>,
    idle: Idle,
    _notification: ExtIdleNotificationV1,
}

impl HostIdle {
    /// Ask the host to tell when no input came for `AWAY`. `None` off
    /// Wayland or on a host without ext-idle-notify.
    pub fn new(window: &Window) -> Option<Self> {
        let RawDisplayHandle::Wayland(display) = window.display_handle().ok()?.as_raw() else {
            return None;
        };
        // Safety: winit's display outlives gerdr's window, which lives as
        // long as gerdr.
        let conn = Connection::from_backend(unsafe {
            Backend::from_foreign_display(display.display.as_ptr().cast())
        });
        let (globals, queue) = registry_queue_init::<Idle>(&conn)
            .map_err(|err| tracing::warn!("host idle: {err}"))
            .ok()?;
        let qh = queue.handle();
        let notifier = globals
            .bind::<ExtIdleNotifierV1, _, _>(&qh, 1..=1, ())
            .map_err(|err| tracing::info!("host idle: {err}"))
            .ok()?;
        let seat = globals
            .bind::<WlSeat, _, _>(&qh, 1..=1, ())
            .map_err(|err| tracing::warn!("host idle: {err}"))
            .ok()?;
        let ms = u32::try_from(AWAY.as_millis()).unwrap_or(u32::MAX);
        let notification = notifier.get_idle_notification(ms, &seat, &qh, ());
        if let Err(err) = conn.flush() {
            tracing::warn!("host idle: {err}");
        }
        Some(Self {
            queue,
            idle: Idle(false),
            _notification: notification,
        })
    }

    /// Whether no input came on the host for `AWAY`, as last told.
    pub fn away(&mut self) -> bool {
        if let Err(err) = self.queue.dispatch_pending(&mut self.idle) {
            tracing::warn!("host idle: {err}");
        }
        self.idle.0
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for Idle {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => state.0 = true,
            ext_idle_notification_v1::Event::Resumed => state.0 = false,
            _ => {}
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Idle {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

wayland_client::delegate_noop!(Idle: ignore WlSeat);
wayland_client::delegate_noop!(Idle: ExtIdleNotifierV1);
