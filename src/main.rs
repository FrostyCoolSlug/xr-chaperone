mod app_state;
mod boundary;
mod config;
mod mesh;
mod monado;
mod renderer;
mod ui;
mod ui_canvas;
mod xr_session;
mod xr_thread;

use crate::app_state::XRState;
use crate::monado::set_initial_offset;
use anyhow::{Result, bail};
use app_state::AppState;
use config::Config;
use std::sync::Arc;
use tracing::{error, info};
use xdg::BaseDirectories;

const APP_NAME: &str = env!("CARGO_PKG_NAME");
const CONFIG_FILE: &str = "chaperone.toml";

/// Entry point, loads the config, starts the xr thread, spawns the interface
fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::new("xr_chaperone=debug,iced=debug,app_state=debug,xr_thread=debug,boundary=debug,renderer=debug,mesh=debug,ui=debug")
        )
        .init();

    let xdg_dirs = BaseDirectories::with_prefix(APP_NAME);
    let cfg_path = xdg_dirs.place_config_file(CONFIG_FILE);
    if let Err(e) = cfg_path {
        ui::error(format!("Unable to create config file: {}", e))?;
        bail!(e);
    }
    let cfg_path = cfg_path?;
    let (cfg, initial_polygon) = match Config::load(&cfg_path) {
        Ok(c) => {
            let poly = c.polygon();
            info!("Loaded config, {} boundary points", poly.len());
            (c, Some(poly))
        }
        Err(_) => {
            info!("No config found, starting setup wizard.");
            (Config::default(), None)
        }
    };

    let state = AppState::new();
    if let Some(poly) = initial_polygon {
        let mut s = state.lock();
        s.polygon = poly;
        s.phase = app_state::Phase::Active;
    }

    // Spawn XR render thread
    let xr_state = Arc::clone(&state);
    let xr_cfg = cfg.clone();
    std::thread::Builder::new()
        .name("xr-render".into())
        .spawn(move || xr_thread::run_xr_thread(xr_state, xr_cfg))?;

    // Wait until the XR thread says we're running before deciding what to show
    while state.lock().xr_state == XRState::Starting {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let xr_state = state.lock().xr_state.clone();

    match xr_state {
        XRState::Starting => {}
        XRState::Error(err) => {
            ui::error(err)?;
        }
        XRState::Running => {
            // Load the offsets into monado if configured
            if let Some(offset) = cfg.headset_offset.clone()
                && let Err(e) = set_initial_offset(offset)
            {
                error!("Failed to Set Offsets: {}", e);
            }

            ui::run(state, cfg, cfg_path)?;
        }
    }

    Ok(())
}
