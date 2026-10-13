//! Bridge the clipboard and primary selection between gerdr's clients and the
//! host compositor, using the host's wlr-data-control protocol (no focus or
//! input serials needed). gerdr never copies the data itself: the pipe fd of
//! whoever pastes is handed straight to whoever owns the selection.

use std::{
    os::fd::{AsFd, OwnedFd},
    sync::Mutex,
};

use calloop_wayland_source::WaylandSource;
use smithay::{
    reexports::calloop::LoopHandle,
    wayland::selection::{
        SelectionTarget,
        data_device::{
            clear_data_device_selection, request_data_device_client_selection,
            set_data_device_selection,
        },
        primary_selection::{
            clear_primary_selection, request_primary_client_selection, set_primary_selection,
        },
    },
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, event_created_child,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};

use crate::Gerdr;

/// Mime types announced by a host offer.
type OfferMimes = Mutex<Vec<String>>;

pub struct HostClipboard {
    conn: Connection,
    qh: QueueHandle<Gerdr>,
    manager: ZwlrDataControlManagerV1,
    device: ZwlrDataControlDeviceV1,
    /// Host selection changes seen in the current dispatch; applied once it ends,
    /// so a `cancelled` for our own source arriving in the same batch is seen first.
    pending: Vec<(SelectionTarget, Option<ZwlrDataControlOfferV1>)>,
    /// Host offers currently mirrored to our clients.
    clipboard_offer: Option<ZwlrDataControlOfferV1>,
    primary_offer: Option<ZwlrDataControlOfferV1>,
    /// Sources exporting our clients' selections to the host.
    clipboard_source: Option<ZwlrDataControlSourceV1>,
    primary_source: Option<ZwlrDataControlSourceV1>,
}

impl HostClipboard {
    /// Connect to the host compositor. Returns `None` (clipboard stays local)
    /// if the host lacks wlr-data-control.
    pub fn init(loop_handle: &LoopHandle<'static, Gerdr>) -> Option<Self> {
        let conn = Connection::connect_to_env()
            .map_err(|e| tracing::warn!("host clipboard: cannot connect to host: {e}"))
            .ok()?;
        let (globals, queue) = registry_queue_init::<Gerdr>(&conn).ok()?;
        let qh = queue.handle();
        let manager: ZwlrDataControlManagerV1 = globals
            .bind(&qh, 1..=2, ())
            .map_err(|e| tracing::warn!("host clipboard: no wlr-data-control on host: {e}"))
            .ok()?;
        let seat: WlSeat = globals.bind(&qh, 1..=1, ()).ok()?;
        let device = manager.get_data_device(&seat, &qh, ());
        WaylandSource::new(conn.clone(), queue)
            .insert(loop_handle.clone())
            .ok()?;

        Some(Self {
            conn,
            qh,
            manager,
            device,
            pending: Vec::new(),
            clipboard_offer: None,
            primary_offer: None,
            clipboard_source: None,
            primary_source: None,
        })
    }

    fn offer_slot(&mut self, target: SelectionTarget) -> &mut Option<ZwlrDataControlOfferV1> {
        match target {
            SelectionTarget::Clipboard => &mut self.clipboard_offer,
            SelectionTarget::Primary => &mut self.primary_offer,
        }
    }

    fn source_slot(&mut self, target: SelectionTarget) -> &mut Option<ZwlrDataControlSourceV1> {
        match target {
            SelectionTarget::Clipboard => &mut self.clipboard_source,
            SelectionTarget::Primary => &mut self.primary_source,
        }
    }
}

impl Gerdr {
    /// One of our clients took the selection: offer it to the host too.
    pub fn export_selection(&mut self, target: SelectionTarget, mime_types: Vec<String>) {
        let Some(clip) = self.host_clipboard.as_mut() else {
            return;
        };
        if target == SelectionTarget::Primary && clip.manager.version() < 2 {
            return;
        }
        let source = clip.manager.create_data_source(&clip.qh, target);
        for mime in mime_types {
            source.offer(mime);
        }
        match target {
            SelectionTarget::Clipboard => clip.device.set_selection(Some(&source)),
            SelectionTarget::Primary => clip.device.set_primary_selection(Some(&source)),
        }
        if let Some(old) = clip.source_slot(target).replace(source) {
            old.destroy();
        }
        // The host offer we were mirroring is stale now.
        if let Some(old) = clip.offer_slot(target).take() {
            old.destroy();
        }
        let _ = clip.conn.flush();
    }

    /// One of our clients pastes a selection that came from the host.
    pub fn import_selection(&mut self, target: SelectionTarget, mime_type: String, fd: OwnedFd) {
        let Some(clip) = self.host_clipboard.as_mut() else {
            return;
        };
        if let Some(offer) = clip.offer_slot(target) {
            offer.receive(mime_type, fd.as_fd());
            let _ = clip.conn.flush();
        }
    }

    fn apply_host_selections(&mut self) {
        let Some(clip) = self.host_clipboard.as_mut() else {
            return;
        };
        for (target, offer) in std::mem::take(&mut clip.pending) {
            if clip.source_slot(target).is_some() {
                // The host selection is the one we exported: our own echo.
                if let Some(offer) = offer {
                    offer.destroy();
                }
                continue;
            }
            let mimes = offer
                .as_ref()
                .map(|o| o.data::<OfferMimes>().unwrap().lock().unwrap().clone());
            if let Some(old) = std::mem::replace(clip.offer_slot(target), offer) {
                old.destroy();
            }
            let (dh, seat) = (&self.display_handle, &self.seat);
            match (target, mimes) {
                (SelectionTarget::Clipboard, Some(mimes)) => {
                    set_data_device_selection(dh, seat, mimes, ())
                }
                (SelectionTarget::Primary, Some(mimes)) => {
                    set_primary_selection(dh, seat, mimes, ())
                }
                (SelectionTarget::Clipboard, None) => clear_data_device_selection(dh, seat),
                (SelectionTarget::Primary, None) => clear_primary_selection(dh, seat),
            }
        }
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for Gerdr {
    fn event(
        state: &mut Self,
        _device: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let (target, offer) = match event {
            zwlr_data_control_device_v1::Event::Selection { id } => {
                (SelectionTarget::Clipboard, id)
            }
            zwlr_data_control_device_v1::Event::PrimarySelection { id } => {
                (SelectionTarget::Primary, id)
            }
            _ => return,
        };
        let Some(clip) = state.host_clipboard.as_mut() else {
            return;
        };
        if clip.pending.is_empty() {
            state
                .loop_handle
                .insert_idle(|state| state.apply_host_selections());
        }
        clip.pending.push((target, offer));
    }

    event_created_child!(Gerdr, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, OfferMimes::default()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, OfferMimes> for Gerdr {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        mimes: &OfferMimes,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            mimes.lock().unwrap().push(mime_type);
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, SelectionTarget> for Gerdr {
    fn event(
        state: &mut Self,
        source: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        target: &SelectionTarget,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // A host app pastes our client's selection.
            zwlr_data_control_source_v1::Event::Send { mime_type, fd } => {
                let res = match target {
                    SelectionTarget::Clipboard => {
                        request_data_device_client_selection(&state.seat, mime_type, fd)
                            .map_err(|e| format!("{e:?}"))
                    }
                    SelectionTarget::Primary => {
                        request_primary_client_selection(&state.seat, mime_type, fd)
                            .map_err(|e| format!("{e:?}"))
                    }
                };
                if let Err(err) = res {
                    tracing::debug!("host paste failed: {err:?}");
                }
            }
            // A host app took the selection.
            zwlr_data_control_source_v1::Event::Cancelled => {
                if let Some(clip) = state.host_clipboard.as_mut() {
                    let slot = clip.source_slot(*target);
                    if slot.as_ref() == Some(source) {
                        *slot = None;
                    }
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrDataControlManagerV1, ()> for Gerdr {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlManagerV1,
        _: <ZwlrDataControlManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for Gerdr {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Gerdr {
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
