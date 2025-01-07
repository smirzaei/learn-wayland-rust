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
        wl_registry::{Event, WlRegistry},
        wl_shm::{Format, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::XdgToplevel,
    xdg_wm_base::{self, XdgWmBase},
};

struct AppState {
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    xdg_wm_base: Option<XdgWmBase>,
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
            _ => {}
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            compositor: None,
            shm: None,
            xdg_wm_base: None,
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

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    info!("Starting the application");
    let conn = Connection::connect_to_env()?;
    let display = conn.display();

    let mut event_queue = conn.new_event_queue::<AppState>();
    let qh = event_queue.handle();
    let _registry = display.get_registry(&qh, ());
    let mut state = AppState::default();

    event_queue.roundtrip(&mut state)?;

    let surface = state.compositor.as_ref().unwrap().create_surface(&qh, ());
    let width = 500;
    let height = 500;
    let stride = width * 4; // 4 bytes per pixel
    let size = stride * height;
    let (shm_file, shm_ptr) = create_shm_pool(size)?;

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

    let pool = &state.shm.as_ref().unwrap().create_pool(
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

    surface.attach(Some(&buffer), 0, 0);

    let xdg_wm_base = state.xdg_wm_base.as_ref().unwrap();
    let xdg_surface = xdg_wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();

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
        _state: &mut Self,
        proxy: &XdgSurface,
        event: <XdgSurface as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            // TODO: feels incomplete
            proxy.ack_configure(serial);
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
            Event::Global {
                name,
                interface,
                version,
            } => {
                if interface.starts_with("wl") {
                    info!(?name, ?interface, version, "new global event")
                }

                state.handle_global_add(registry, name, &interface, version, qh);
            }
            Event::GlobalRemove { name } => todo!(),
            _ => unreachable!(),
        }
    }
}
