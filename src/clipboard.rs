//! Bridge the clipboard and primary selection between gerdr's clients and the
//! host compositor, using the host's wlr-data-control protocol (no focus or
//! input serials needed). gerdr never copies the data itself: the pipe fd of
//! whoever pastes is handed straight to whoever owns the selection. Only
//! gerdr's own text fields (see `text_field`) read the clipboard, off the
//! event loop, and own it with the text copied from them.

use std::{
    fs::File,
    io::{Read, Write},
    os::fd::{AsFd, OwnedFd},
    sync::Mutex,
};

use calloop_wayland_source::WaylandSource;
use smithay::{
    reexports::calloop::{LoopHandle, channel},
    wayland::selection::{
        SelectionTarget,
        data_device::{
            SelectionRequestError, clear_data_device_selection,
            request_data_device_client_selection, set_data_device_selection,
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

use crate::{Gerdr, input::Overlay};

/// Mime types announced by a host offer.
type OfferMimes = Mutex<Vec<String>>;

/// What a paste brings back to the event loop: the overlay it is for, and
/// what to put into it or why an image couldn't be pasted.
pub type Pasted = (Overlay, Result<Paste, String>);

/// What a paste puts into a field.
#[derive(Debug)]
pub enum Paste {
    /// Text, typed into it.
    Text(String),
    /// Files attached to an agent's chat: copied in a file manager, or an
    /// image saved to a file.
    Files(Vec<std::path::PathBuf>),
}

/// The mime type of files copied in a file manager.
const URI_LIST: &str = "text/uri-list";

/// The text mime types gerdr pastes, the most wanted first.
const TEXT_MIMES: [&str; 4] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
];

/// The most of the clipboard gerdr reads for a paste.
const PASTE_LIMIT: u64 = 64 * 1024;

/// The most of an image gerdr reads for a paste into a chat.
const IMAGE_LIMIT: u64 = 20 * 1024 * 1024;

/// The mime types a paste into overlay `into` asks for, the most wanted
/// first: text; in an agent's chat, files copied in a file manager before
/// it and an image after it (see `chat::files`, `chat::images`).
fn paste_mimes(into: Overlay) -> impl Iterator<Item = &'static str> {
    let chat = into == Overlay::Chat;
    let files = std::iter::once(URI_LIST).filter(move |_| chat);
    let images = crate::chat::images::MIMES.into_iter().filter(move |_| chat);
    files.chain(TEXT_MIMES).chain(images)
}

/// Whether `mime` is one of an image.
fn is_image(mime: &str) -> bool {
    mime.starts_with("image/")
}

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
        if target == SelectionTarget::Clipboard {
            self.notices.copied = None;
        }
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

    /// Put `text`, copied from one of gerdr's own text fields, on the
    /// clipboard: for our clients, and for the host's through a source of
    /// our own.
    pub fn copy_text(&mut self, text: String) {
        let mimes: Vec<String> = TEXT_MIMES.iter().map(|m| (*m).to_owned()).collect();
        self.export_selection(SelectionTarget::Clipboard, mimes.clone());
        set_data_device_selection(&self.display_handle, &self.seat, mimes, ());
        self.notices.copied = Some(text);
    }

    /// One of our clients pastes a selection that came from the host, or
    /// the text copied from gerdr's own fields.
    pub fn import_selection(&mut self, target: SelectionTarget, mime_type: String, fd: OwnedFd) {
        if target == SelectionTarget::Clipboard
            && let Some(text) = &self.notices.copied
        {
            return write_copied(text, fd);
        }
        let Some(clip) = self.host_clipboard.as_mut() else {
            return;
        };
        if let Some(offer) = clip.offer_slot(target) {
            offer.receive(mime_type, fd.as_fd());
            let _ = clip.conn.flush();
        }
    }

    /// Read the clipboard's text off the event loop (its owner may be one of
    /// our clients, which needs the loop to answer), for `paste_text` into
    /// overlay `into`: not into one opened later, when the owner is slow.
    /// Nothing comes back when the owner writes nothing. In a chat, files
    /// copied in a file manager are attached, and so is an image pasted,
    /// saved to a file first.
    pub fn paste_clipboard(&mut self, into: Overlay) {
        let Some(tx) = self.paste_sender() else {
            return;
        };
        let (mut reader, writer) = match std::io::pipe() {
            Ok(pipe) => pipe,
            Err(err) => return tracing::warn!("paste: {err}"),
        };
        let Some(mime) = self.request_clipboard(into, writer.into()) else {
            return;
        };
        let image = is_image(mime);
        std::thread::spawn(move || {
            let limit = if image { IMAGE_LIMIT } else { PASTE_LIMIT };
            let mut bytes = Vec::new();
            // One byte past the limit tells an image too big from one that fits.
            match (&mut reader)
                .take(limit + u64::from(image))
                .read_to_end(&mut bytes)
            {
                Ok(_) if bytes.is_empty() => {}
                Ok(_) if image && bytes.len() as u64 > limit => {
                    let most = IMAGE_LIMIT / 1024 / 1024;
                    let _ = tx.send((into, Err(format!("it is over {most} MB"))));
                }
                Ok(_) if image => {
                    let saved = crate::chat::images::save(&bytes);
                    let _ = tx.send((into, saved.map(|path| Paste::Files(vec![path]))));
                }
                Ok(_) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    let paths = match mime == URI_LIST {
                        true => crate::chat::files::uri_paths(&text),
                        false => Vec::new(),
                    };
                    let paste = match paths.is_empty() {
                        true => Paste::Text(text),
                        false => Paste::Files(paths),
                    };
                    let _ = tx.send((into, Ok(paste)));
                }
                Err(err) if image => _ = tx.send((into, Err(err.to_string()))),
                Err(err) => tracing::debug!("paste: {err}"),
            }
        });
    }

    /// Ask whoever owns the clipboard (one of our clients or, through its
    /// mirror, the host) to write what overlay `into` takes into `fd`; the
    /// mime type asked for. None if no one has any.
    fn request_clipboard(&mut self, into: Overlay, fd: OwnedFd) -> Option<&'static str> {
        for mime in paste_mimes(into) {
            let dup = fd.try_clone().ok()?;
            match request_data_device_client_selection(&self.seat, mime.to_owned(), dup) {
                Ok(()) => return Some(mime),
                Err(SelectionRequestError::InvalidMimetype) => {}
                Err(SelectionRequestError::ServerSideSelection) => {
                    self.import_selection(SelectionTarget::Clipboard, mime.to_owned(), fd);
                    return Some(mime);
                }
                Err(SelectionRequestError::NoSelection) => return None,
            }
        }
        None
    }

    /// Where the clipboard's text read for a paste goes back to the event
    /// loop, with the overlay it is for; or why an image couldn't be.
    fn paste_sender(&mut self) -> Option<channel::Sender<Pasted>> {
        if self.notices.paste.is_none() {
            let (tx, rx) = channel::channel::<Pasted>();
            let inserted = self
                .loop_handle
                .insert_source(rx, |event, _, state| match event {
                    channel::Event::Msg((into, Ok(Paste::Text(text)))) => {
                        state.paste_text(into, &text)
                    }
                    channel::Event::Msg((_, Ok(Paste::Files(paths)))) => {
                        state.chat_attach_focused(paths)
                    }
                    channel::Event::Msg((_, Err(why))) => {
                        tracing::warn!("paste of an image: {why}");
                        state.show_error(format!("Couldn't paste the image: {why}"));
                    }
                    channel::Event::Closed => {}
                });
            match inserted {
                Ok(_) => self.notices.paste = Some(tx),
                Err(err) => tracing::warn!("paste: {err}"),
            }
        }
        self.notices.paste.clone()
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
                .and_then(|o| o.data::<OfferMimes>())
                .map(|m| m.lock().expect("offer mimes lock poisoned").clone());
            if let Some(old) = std::mem::replace(clip.offer_slot(target), offer) {
                old.destroy();
            }
            if target == SelectionTarget::Clipboard {
                // A host app took the clipboard from gerdr's copy.
                self.notices.copied = None;
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

/// Write `text`, copied from one of gerdr's fields, into the pipe `fd` of
/// whoever pastes it, off the event loop.
fn write_copied(text: &str, fd: OwnedFd) {
    let text = text.to_owned();
    std::thread::spawn(move || {
        if let Err(err) = File::from(fd).write_all(text.as_bytes()) {
            tracing::debug!("copy: {err}");
        }
    });
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
            mimes
                .lock()
                .expect("offer mimes lock poisoned")
                .push(mime_type);
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
            // A host app pastes our client's selection, or our own copy.
            zwlr_data_control_source_v1::Event::Send { mime_type, fd } => {
                if *target == SelectionTarget::Clipboard
                    && let Some(text) = &state.notices.copied
                {
                    return write_copied(text, fd);
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_chat_asks_for_files_before_text_and_an_image_after_it() {
        let chat: Vec<_> = paste_mimes(Overlay::Chat).collect();
        assert_eq!(chat[..2], [URI_LIST, TEXT_MIMES[0]]);
        assert_eq!(chat.last(), Some(&"image/jpeg"));
        assert!(chat.iter().position(|m| is_image(m)) > chat.iter().position(|m| *m == "STRING"));
        let other: Vec<_> = paste_mimes(Overlay::Features).collect();
        assert_eq!(other, TEXT_MIMES);
    }
}
