use anyhow::{Context, Result};
use log::{error, warn};
use niri_ipc::socket::Socket;
use niri_ipc::{Action, Request, Response, Window};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct NiriState {
    pub windows: Vec<Window>,
    pub output_widths: HashMap<String, f64>,
    pub ws_outputs: HashMap<u64, String>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct WindowPosition {
    pub workspace_id: u64,
    pub column: usize,
    pub tile: usize,
}

pub trait NiriConnection: Send {
    fn send_action(&mut self, action: Action) -> Result<()>;
    fn query_focused_window(&mut self) -> Result<Option<u64>>;
    fn query_full_state(&mut self) -> Result<NiriState>;
}

pub struct SocketConnection {
    socket: Socket,
}

impl SocketConnection {
    pub fn new() -> Result<Self> {
        let socket = Socket::connect().context("connecting to niri via socket")?;
        Ok(Self { socket })
    }
}

impl NiriConnection for SocketConnection {
    fn send_action(&mut self, action: Action) -> Result<()> {
        let reply = self
            .socket
            .send(Request::Action(action.clone()))
            .context("sending action to niri")?;
        match reply {
            Ok(Response::Handled) => Ok(()),
            Ok(other) => {
                warn!(
                    "unexpected response from niri for action {:?}: {:?}",
                    action, other
                );
                Ok(())
            }
            Err(msg) => {
                error!("niri returned error for action {:?}: {}", action, msg);
                Ok(())
            }
        }
    }

    fn query_focused_window(&mut self) -> Result<Option<u64>> {
        let reply = self
            .socket
            .send(Request::FocusedWindow)
            .context("querying focused window")?;
        match reply {
            Ok(Response::FocusedWindow(Some(w))) => Ok(Some(w.id)),
            Ok(Response::FocusedWindow(None)) => Ok(None),
            _ => {
                warn!("unexpected response when querying focused window");
                Ok(None)
            }
        }
    }

    fn query_full_state(&mut self) -> Result<NiriState> {
        let windows = match self
            .socket
            .send(Request::Windows)
            .context("querying windows")?
        {
            Ok(Response::Windows(w)) => w,
            _ => anyhow::bail!("failed to query windows"),
        };

        let output_widths = match self
            .socket
            .send(Request::Outputs)
            .context("querying outputs")?
        {
            Ok(Response::Outputs(outputs)) => {
                let mut widths = HashMap::new();
                for (name, out) in outputs {
                    if let Some(logical) = out.logical {
                        if logical.width > 0 {
                            widths.insert(name, logical.width as f64);
                        } else {
                            warn!("output {} has non-positive width: {}", name, logical.width);
                        }
                    }
                }
                widths
            }
            _ => anyhow::bail!("failed to query outputs"),
        };

        let ws_outputs = match self
            .socket
            .send(Request::Workspaces)
            .context("querying workspaces")?
        {
            Ok(Response::Workspaces(workspaces)) => {
                let mut mapping = HashMap::new();
                for ws in workspaces {
                    if let Some(output) = ws.output {
                        mapping.insert(ws.id, output);
                    }
                }
                mapping
            }
            _ => anyhow::bail!("failed to query workspaces"),
        };

        Ok(NiriState {
            windows,
            output_widths,
            ws_outputs,
        })
    }
}
