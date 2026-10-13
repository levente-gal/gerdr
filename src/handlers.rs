//! Wayland protocol plumbing.

use std::os::fd::OwnedFd;

use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        renderer::{ImportDma, utils::on_commit_buffer_handler},
    },
    delegate_compositor, delegate_cursor_shape, delegate_data_control, delegate_data_device,
    delegate_dmabuf, delegate_ext_data_control, delegate_fractional_scale, delegate_output,
    delegate_primary_selection, delegate_seat, delegate_shm, delegate_viewporter,
    delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{PopupKind, Window, find_popup_root_surface, get_popup_toplevel_coords},
    input::{
        Seat, SeatHandler, SeatState,
        pointer::{CursorIcon, CursorImageStatus},
    },
    reexports::{
        wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
        wayland_server::{
            Client, Resource,
            protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{Point, Serial},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface, with_states,
        },
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        fractional_scale::FractionalScaleHandler,
        output::OutputHandler,
        selection::{
            SelectionHandler, SelectionSource, SelectionTarget,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
            ext_data_control::{
                DataControlHandler as ExtDataControlHandler,
                DataControlState as ExtDataControlState,
            },
            primary_selection::{
                PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
            },
            wlr_data_control::{DataControlHandler, DataControlState},
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData, decoration::XdgDecorationHandler,
        },
        shm::{ShmHandler, ShmState},
        tablet_manager::TabletSeatHandler,
    },
};

use crate::{ClientState, Gerdr, set_preferred_scale};

impl CompositorHandler for Gerdr {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        set_preferred_scale(surface, self.scale());

        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(&root) {
                window.on_commit();
            }
        }

        // First commit of a toplevel: app_id is known now, so pick its tab and pane.
        if let Some(window) = self
            .unplaced
            .iter()
            .find(|w| w.toplevel().unwrap().wl_surface() == surface)
            .cloned()
        {
            let (app_id, configured) = with_states(surface, |states| {
                let data = states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap();
                (
                    data.app_id.clone().unwrap_or_default(),
                    data.initial_configure_sent,
                )
            });
            if !configured {
                self.place(window.clone(), &app_id);
                let toplevel = window.toplevel().unwrap();
                if !toplevel.is_initial_configure_sent() {
                    toplevel.send_configure();
                }
            }
        }

        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
        {
            popup.send_configure().expect("initial configure failed");
        }
    }
}

impl BufferHandler for Gerdr {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Gerdr {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl DmabufHandler for Gerdr {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        if self.backend.renderer().import_dmabuf(&dmabuf, None).is_ok() {
            let _ = notifier.successful::<Gerdr>();
        } else {
            notifier.failed();
        }
    }
}

impl XdgShellHandler for Gerdr {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.unplaced.push(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.remove_window(surface.wl_surface());
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    // Panes are fixed: ignore move/resize/fullscreen requests.
    fn move_request(&mut self, _surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
}

impl Gerdr {
    /// Keep popups (menus, completions) inside the window they belong to.
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self.window_for_surface(&root) else {
            return;
        };
        let Some(window_geo) = self.space.element_geometry(&window) else {
            return;
        };
        let mut target = window_geo;
        target.loc = Point::default() - get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

impl XdgDecorationHandler for Gerdr {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel
            .with_pending_state(|state| state.decoration_mode = Some(DecorationMode::ServerSide));
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        self.new_decoration(toplevel.clone());
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.request_mode(toplevel, DecorationMode::ServerSide);
    }
}

impl SeatHandler for Gerdr {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Gerdr> {
        &mut self.seat_state
    }

    // Show the client's requested cursor shape on the host window.
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        let window = self.backend.window();
        match image {
            CursorImageStatus::Hidden => window.set_cursor_visible(false),
            CursorImageStatus::Named(icon) => {
                window.set_cursor_visible(true);
                window.set_cursor(icon);
            }
            CursorImageStatus::Surface(_) => {
                window.set_cursor_visible(true);
                window.set_cursor(CursorIcon::Default);
            }
        }
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);
    }
}

impl TabletSeatHandler for Gerdr {}

impl SelectionHandler for Gerdr {
    type SelectionUserData = ();

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        if let Some(source) = source {
            self.export_selection(ty, source.mime_types());
        }
    }

    // Only reached for selections we set ourselves, i.e. ones mirrored from the host.
    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        _: &(),
    ) {
        self.import_selection(ty, mime_type, fd);
    }
}

impl PrimarySelectionHandler for Gerdr {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

impl DataDeviceHandler for Gerdr {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl DataControlHandler for Gerdr {
    fn data_control_state(&self) -> &DataControlState {
        &self.data_control_state
    }
}

impl ExtDataControlHandler for Gerdr {
    fn data_control_state(&self) -> &ExtDataControlState {
        &self.ext_data_control_state
    }
}

impl ClientDndGrabHandler for Gerdr {}
impl ServerDndGrabHandler for Gerdr {}
impl OutputHandler for Gerdr {}
impl FractionalScaleHandler for Gerdr {}

delegate_compositor!(Gerdr);
delegate_shm!(Gerdr);
delegate_dmabuf!(Gerdr);
delegate_xdg_shell!(Gerdr);
delegate_xdg_decoration!(Gerdr);
delegate_seat!(Gerdr);
delegate_data_device!(Gerdr);
delegate_primary_selection!(Gerdr);
delegate_data_control!(Gerdr);
delegate_ext_data_control!(Gerdr);
delegate_output!(Gerdr);
delegate_viewporter!(Gerdr);
delegate_fractional_scale!(Gerdr);
delegate_cursor_shape!(Gerdr);
