use anyhow::{Context, Result};
use log::{debug, error, info};
use niri_ipc::socket::Socket;
use niri_ipc::{Event, Request};
use std::collections::HashMap;
use std::env;

mod connection;
mod manager;

#[cfg(test)]
mod tests;

use crate::connection::SocketConnection;
use crate::manager::NiriContext;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage: niritiling [OPTIONS]");
        println!();
        println!("Options:");
        println!(
            "  --resize-columns          When a column is resized, adjust the other column to compensate"
        );
        println!(
            "  --columns <IDENTITY>=<N>  Size columns to fit up to N side by side on the output"
        );
        println!(
            "                            with this identity (\"make model serial\"); repeatable."
        );
        println!("                            N=1 keeps the output at one column and stacks newly");
        println!("                            opened windows into it");
        println!(
            "  --default-columns <N>     When a workspace moves to an output without a --columns"
        );
        println!("                            entry, size its columns to fit up to N side by side");
        println!("                            (default: 2)");
        println!("  -h, --help                Show this help message");
        return Ok(());
    }

    let resize_columns = args.iter().any(|a| a == "--resize-columns");
    let column_targets = parse_column_targets(&args)?;
    let default_columns = parse_default_columns(&args)?;

    info!(
        "niritiling: starting (resize_columns={}, column_targets={:?}, default_columns={})",
        resize_columns, column_targets, default_columns
    );

    loop {
        if let Err(e) = run_event_loop(resize_columns, &column_targets, default_columns) {
            error!(
                "fatal error in event loop: {:?}. attempting to reconnect in 5 seconds...",
                e
            );
            std::thread::sleep(std::time::Duration::from_secs(5));
        } else {
            info!("event loop exited normally. restarting...");
        }
    }
}

fn parse_column_targets(args: &[String]) -> Result<HashMap<String, usize>> {
    let mut targets = HashMap::new();
    for (i, arg) in args.iter().enumerate() {
        if arg != "--columns" {
            continue;
        }
        let (identity, count) = args
            .get(i + 1)
            .and_then(|v| v.rsplit_once('='))
            .and_then(|(identity, n)| Some((identity.to_string(), n.parse().ok()?)))
            .context("--columns requires a value of the form \"<identity>=<N>\"")?;
        targets.insert(identity, count);
    }
    Ok(targets)
}

fn parse_default_columns(args: &[String]) -> Result<usize> {
    match args.iter().position(|a| a == "--default-columns") {
        Some(i) => args
            .get(i + 1)
            .and_then(|n| n.parse().ok())
            .filter(|&n| n > 0)
            .context("--default-columns requires a positive integer"),
        None => Ok(2),
    }
}

fn run_event_loop(
    resize_columns: bool,
    column_targets: &HashMap<String, usize>,
    default_columns: usize,
) -> Result<()> {
    let conn = SocketConnection::new()?;
    let mut context = NiriContext::new(
        Box::new(conn),
        resize_columns,
        column_targets.clone(),
        default_columns,
    );

    let mut event_socket = Socket::connect().context("connecting to niri event stream")?;
    let _ = event_socket
        .send(Request::EventStream)
        .context("failed to request event stream")?;
    let mut read_event = event_socket.read_events();

    info!("connected to niri; performing initial synchronization");
    let state = context
        .connection
        .query_full_state()
        .context("initial state query failed")?;
    context.seed_outputs(&state);
    context.handle_event(Event::WindowsChanged {
        windows: state.windows,
    })?;

    loop {
        let event = match read_event() {
            Ok(event) => event,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                debug!("skipping unrecognized event from niri: {e}");
                continue;
            }
            Err(e) => return Err(e).context("reading event from niri"),
        };

        if let Err(e) = context.handle_event(event) {
            error!("error handling event: {:?}", e);
            if e.chain()
                .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
            {
                return Err(e);
            }
        }
    }
}
