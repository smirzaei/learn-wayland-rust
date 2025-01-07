#![warn(clippy::all)]
use std::{
    fs::File,
    os::fd::{AsFd, AsRawFd},
    ptr,
};

use anyhow::{bail, Ok};
use tempfile::tempfile;
use tracing::{debug, info};
use wayland_client::{
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_display::WlDisplay,
        wl_registry::{self, WlRegistry},
        wl_shm::{Format, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::xdg::{
    decoration::zv1::client::{
        zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
        zxdg_toplevel_decoration_v1::{self, Mode, ZxdgToplevelDecorationV1},
    },
    shell::client::{
        xdg_surface::{self, XdgSurface},
        xdg_toplevel::XdgToplevel,
        xdg_wm_base::{self, XdgWmBase},
    },
};

struct AppState {
    // Globals
    display: Option<WlDisplay>,
    compositor: Option<WlCompositor>,
    registry: Option<WlRegistry>,
    shm: Option<WlShm>,
    xdg_wm_base: Option<XdgWmBase>,
    xdg_decoration_manager: Option<ZxdgDecorationManagerV1>,

    // Objects
    surface: Option<WlSurface>,
    xdg_surface: Option<XdgSurface>,
    xdg_toplevel: Option<XdgToplevel>,

    queue_handle: Option<QueueHandle<Self>>,
}

impl AppState {
    fn handle_global_add(
        &mut self,
        registry: &WlRegistry,
        name: u32,
        interface: &str,
        version: u32,
        qh: &QueueHandle<Self>,
    ) {
        match interface {
            "wl_compositor" => {
                debug!(?interface, ?name, ?version, "Adding compositor");
                let compositor = registry.bind(name, version, qh, ());
                self.compositor = Some(compositor);
            }
            "wl_shm" => {
                debug!(?interface, ?name, ?version, "Adding shm");
                let shm = registry.bind(name, version, qh, ());
                self.shm = Some(shm);
            }
            "xdg_wm_base" => {
                debug!(?interface, ?name, ?version, "Adding xdg_wm_base");
                let xdg_wm_base = registry.bind(name, version, qh, ());
                self.xdg_wm_base = Some(xdg_wm_base);
            }
            "zxdg_decoration_manager_v1" => {
                debug!(?interface, ?name, ?version, "Adding decoration manager");
                let decoration_manager = registry.bind(name, version.min(1), qh, ());
                self.xdg_decoration_manager = Some(decoration_manager);
            }
            _ => {}
        }
    }

    fn set_display(&mut self, display: WlDisplay) {
        self.display = Some(display);
    }

    fn set_compositor(&mut self, compositor: WlCompositor) {
        self.compositor = Some(compositor);
    }

    fn set_registry(&mut self, registry: WlRegistry) {
        self.registry = Some(registry);
    }

    fn set_surface(&mut self, surface: WlSurface) {
        self.surface = Some(surface);
    }

    fn set_xdg_surface(&mut self, xdg_surface: XdgSurface) {
        self.xdg_surface = Some(xdg_surface);
    }

    fn set_xdg_toplevel(&mut self, xdg_toplevel: XdgToplevel) {
        self.xdg_toplevel = Some(xdg_toplevel);
    }

    fn set_queue_handle(&mut self, qh: QueueHandle<Self>) {
        self.queue_handle = Some(qh);
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            display: None,
            registry: None,
            compositor: None,
            shm: None,
            xdg_wm_base: None,
            xdg_decoration_manager: None,
            surface: None,
            xdg_surface: None,
            xdg_toplevel: None,
            queue_handle: None,
        }
    }
}

fn create_shm_pool(size: usize) -> anyhow::Result<(File, *mut u8)> {
    let tmpfile = tempfile()?;
    tmpfile.set_len(size as u64)?;

    // WARN: what happens to this fd when tmpfile goes out of scope?
    let fd = tmpfile.as_raw_fd();
    unsafe {
        let res = libc::mmap(
            ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        );

        if res == libc::MAP_FAILED {
            bail!("failed to mmap memory");
        }

        Ok((tmpfile, res as *mut u8))
    }
}

fn draw_frame(state: &AppState) -> anyhow::Result<WlBuffer> {
    let qh = state.queue_handle.as_ref().unwrap();

    let width = 500;
    let height = 500;
    let stride = width * 4; // 4 bytes per pixel
    let size = stride * height;
    let (shm_file, shm_ptr) = create_shm_pool(size)?;

    let pool = state.shm.as_ref().unwrap().create_pool(
        shm_file.as_fd(),
        size.try_into().unwrap(),
        &qh,
        (),
    );

    let buffer = pool.create_buffer(
        0,
        width.try_into().unwrap(),
        height.try_into().unwrap(),
        stride.try_into().unwrap(),
        Format::Argb8888,
        &qh,
        (),
    );

    unsafe {
        for y in 0..height {
            for x in 0..width {
                let offset = (y * width + x) * 4;
                let pixel = shm_ptr.add(offset);

                // ARGB format
                *pixel = 0xFF; // Alpha
                *pixel.add(1) = 0x00; // Red
                *pixel.add(2) = 0x00; // Green
                *pixel.add(3) = 0xFF; // Blue
            }
        }
    }

    Ok(buffer)
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    info!("Starting the application");

    let mut state = AppState::default();

    let conn = Connection::connect_to_env()?;
    let display = conn.display();
    state.set_display(display);

    let mut event_queue = conn.new_event_queue::<AppState>();
    state.set_queue_handle(event_queue.handle());
    let qh = event_queue.handle();

    let registry = state.display.as_ref().unwrap().get_registry(&qh, ());
    state.set_registry(registry);

    event_queue.roundtrip(&mut state)?;

    let surface = state.compositor.as_ref().unwrap().create_surface(&qh, ());
    state.set_surface(surface);

    let xdg_wm_base = state.xdg_wm_base.as_ref().unwrap();
    let xdg_surface = xdg_wm_base.get_xdg_surface(state.surface.as_ref().unwrap(), &qh, ());
    state.set_xdg_surface(xdg_surface);

    let toplevel = state.xdg_surface.as_ref().unwrap().get_toplevel(&qh, ());
    toplevel.set_title(String::from("Hello, world!"));
    let decoration_manager = state.xdg_decoration_manager.as_ref().unwrap();
    let decoration = decoration_manager.get_toplevel_decoration(&toplevel, &qh, ());
    decoration.set_mode(Mode::ServerSide);

    state.set_xdg_toplevel(toplevel);

    state.surface.as_ref().unwrap().commit();

    loop {
        event_queue.blocking_dispatch(&mut state)?;
    }
}

impl Dispatch<WlSurface, ()> for AppState {
    fn event(
        _state: &mut Self,
        _proxy: &WlSurface,
        _event: <WlSurface as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlCompositor, ()> for AppState {
    fn event(
        _state: &mut Self,
        _proxy: &WlCompositor,
        _event: <WlCompositor as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        // This interface does not generates any events AFAIK
    }
}

impl Dispatch<WlShm, ()> for AppState {
    fn event(
        _state: &mut Self,
        _proxy: &WlShm,
        _event: <WlShm as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // AFAIK, this interface only sends events advertising the supported
        // pixel formats.
    }
}

impl Dispatch<WlShmPool, ()> for AppState {
    fn event(
        _state: &mut Self,
        _proxy: &WlShmPool,
        _event: <WlShmPool as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // AFAIK, this interface does not emit any events
    }
}

impl Dispatch<WlBuffer, ()> for AppState {
    fn event(
        _state: &mut Self,
        _proxy: &WlBuffer,
        _event: <WlBuffer as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // TODO: when the compositor is done using the buffer, it will emit a `release` event.
        // I need to release ro re-use the buffer after receiving that event.
        // wayland_client::protocol::wl_buffer::Event::Release
    }
}

impl Dispatch<XdgWmBase, ()> for AppState {
    fn event(
        _state: &mut Self,
        proxy: &XdgWmBase,
        event: <XdgWmBase as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            debug!(?serial, "xdg ping");
            proxy.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for AppState {
    fn event(
        state: &mut Self,
        proxy: &XdgSurface,
        event: <XdgSurface as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            info!(?serial, "xdg surface configure event");
            proxy.ack_configure(serial);

            let toplevel = state.xdg_toplevel.as_ref().unwrap();
            let qh = state.queue_handle.as_ref().unwrap();
            let surface = state.surface.as_ref().unwrap();
            let buffer = draw_frame(state).expect("failed to draw frame");

            surface.attach(Some(&buffer), 0, 0);
            surface.commit();
        }
    }
}

impl Dispatch<XdgToplevel, ()> for AppState {
    fn event(
        _state: &mut Self,
        _proxy: &XdgToplevel,
        _event: <XdgToplevel as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // TODO: Handle window state changes
    }
}

impl Dispatch<WlRegistry, ()> for AppState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: <WlRegistry as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qh: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                if interface.starts_with("wl") {
                    info!(?name, ?interface, version, "new global event")
                }

                state.handle_global_add(registry, name, &interface, version, qh);
            }
            wl_registry::Event::GlobalRemove { name } => todo!(),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<ZxdgDecorationManagerV1, ()> for AppState {
    fn event(
        _state: &mut Self,
        _proxy: &ZxdgDecorationManagerV1,
        _event: <ZxdgDecorationManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // AFAIK, this interface does not emit any events
    }
}

impl Dispatch<ZxdgToplevelDecorationV1, ()> for AppState {
    fn event(
        _state: &mut Self,
        _proxy: &ZxdgToplevelDecorationV1,
        event: <ZxdgToplevelDecorationV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zxdg_toplevel_decoration_v1::Event::Configure { mode } => {
                info!(?mode, "decoration configure event");
            }
            _ => unreachable!(),
        }
    }
}
