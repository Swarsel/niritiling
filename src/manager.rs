use crate::connection::{NiriConnection, NiriState, WindowPosition};
use anyhow::Result;
use log::{debug, error, info};
use niri_ipc::{Action, Event, Window};
use std::collections::HashMap;

const MAXIMIZED_RATIO_THRESHOLD: f64 = 0.9;

pub struct NiriContext {
    pub connection: Box<dyn NiriConnection>,
    pub tracked_window_positions: HashMap<u64, WindowPosition>,
    pub debounced_maximize_state: HashMap<u64, (bool, std::time::Instant)>,
}

impl NiriContext {
    pub fn new(connection: Box<dyn NiriConnection>) -> Self {
        Self {
            connection,
            tracked_window_positions: HashMap::new(),
            debounced_maximize_state: HashMap::new(),
        }
    }

    fn send_action(&mut self, action: Action) -> Result<()> {
        self.connection.send_action(action)
    }

    fn query_focused_window(&mut self) -> Result<Option<u64>> {
        self.connection.query_focused_window()
    }

    fn query_full_state(&mut self) -> Result<NiriState> {
        self.connection.query_full_state()
    }

    fn is_maximized(
        &self,
        window_id: u64,
        state: &NiriState,
        windows_map: &HashMap<u64, &Window>,
    ) -> bool {
        if let Some(w) = windows_map.get(&window_id) {
            if let Some(ws_id) = w.workspace_id {
                if let Some(output_name) = state.ws_outputs.get(&ws_id) {
                    if let Some(&output_width) = state.output_widths.get(output_name) {
                        if output_width <= 0.0 {
                            return false;
                        }
                        let tile_width = w.layout.tile_size.0;
                        let ratio = tile_width / output_width;
                        debug!(
                            "window {} tile_width={:.0} output_width={:.0} ratio={:.2}",
                            window_id, tile_width, output_width, ratio
                        );
                        return ratio > MAXIMIZED_RATIO_THRESHOLD;
                    }
                }
            }
        }
        false
    }

    fn perform_maximize_action(
        &mut self,
        target_window_id: u64,
        restore_focus: bool,
    ) -> Result<()> {
        let original_focus = self.query_focused_window().ok().flatten();

        if original_focus != Some(target_window_id) {
            self.send_action(Action::FocusWindow {
                id: target_window_id,
            })?;
        }

        self.send_action(Action::MaximizeColumn {})?;

        if restore_focus {
            if let Some(orig_id) = original_focus {
                if orig_id != target_window_id {
                    debug!("restoring focus to {}", orig_id);
                    let _ = self.send_action(Action::FocusWindow { id: orig_id });
                }
            }
        }
        Ok(())
    }

    pub fn evaluate_workspace(
        &mut self,
        ws_id: u64,
        state: &NiriState,
        windows_map: &HashMap<u64, &Window>,
    ) -> Result<()> {
        let tiled_windows: Vec<&Window> = state
            .windows
            .iter()
            .filter(|w| w.workspace_id == Some(ws_id) && !w.is_floating)
            .collect();

        if tiled_windows.is_empty() {
            return Ok(());
        }

        let mut unique_columns = std::collections::HashSet::new();
        for w in &tiled_windows {
            if let Some((col_idx, _)) = w.layout.pos_in_scrolling_layout {
                unique_columns.insert(col_idx);
            }
        }

        let column_count = unique_columns.len();

        if column_count == 1 {
            let win_id = tiled_windows[0].id;
            if !self.is_maximized(win_id, state, windows_map) {
                let now = std::time::Instant::now();
                if let Some(&(target_maximized, last_time)) =
                    self.debounced_maximize_state.get(&win_id)
                {
                    if target_maximized
                        && now.duration_since(last_time) < std::time::Duration::from_millis(200)
                    {
                        debug!(
                            "workspace {}: skipping maximize for window {} due to debounce",
                            ws_id, win_id
                        );
                        return Ok(());
                    }
                }
                self.debounced_maximize_state.insert(win_id, (true, now));

                info!(
                    "workspace {}: single column -> maximizing window {}",
                    ws_id, win_id
                );
                self.perform_maximize_action(win_id, true)?;
            }
        } else {
            let target_nudge_focus = self.query_focused_window().ok().flatten();
            let mut cols_vec: Vec<usize> = unique_columns.into_iter().collect();
            cols_vec.sort_unstable();

            for &col_idx in &cols_vec {
                if let Some(w) = tiled_windows
                    .iter()
                    .find(|w| w.layout.pos_in_scrolling_layout.map(|(c, _)| c) == Some(col_idx))
                {
                    if self.is_maximized(w.id, state, windows_map) {
                        let now = std::time::Instant::now();
                        if let Some(&(target_maximized, last_time)) =
                            self.debounced_maximize_state.get(&w.id)
                        {
                            if !target_maximized
                                && now.duration_since(last_time)
                                    < std::time::Duration::from_millis(200)
                            {
                                debug!(
                                    "workspace {}: skipping un-maximize for window {} due to debounce",
                                    ws_id, w.id
                                );
                                continue;
                            }
                        }
                        self.debounced_maximize_state.insert(w.id, (false, now));

                        info!(
                            "workspace {}: multiple columns -> un-maximizing window {} in column {}",
                            ws_id, w.id, col_idx
                        );
                        self.perform_maximize_action(w.id, false)?;
                    }
                }
            }

            debug!(
                "workspace {}: waiting for layout to settle before viewport nudge",
                ws_id
            );
            std::thread::sleep(std::time::Duration::from_millis(50));

            debug!(
                "workspace {}: nudging viewport left (target focus: {:?})",
                ws_id, target_nudge_focus
            );
            self.send_action(Action::FocusColumnLeft {})?;
            if let Some(orig_id) = target_nudge_focus {
                debug!("workspace {}: restoring focus to {}", ws_id, orig_id);
                let _ = self.send_action(Action::FocusWindow { id: orig_id });
            }
        }
        Ok(())
    }

    pub fn handle_event(&mut self, event: Event) -> Result<()> {
        let mut affected_workspaces = Vec::new();

        match event {
            Event::WindowsChanged { windows } => {
                debug!("full windows change event received");
                let mut new_tracked = HashMap::with_capacity(windows.len());

                for w in windows {
                    if !w.is_floating {
                        if let Some(ws_id) = w.workspace_id {
                            if let Some((col, tile)) = w.layout.pos_in_scrolling_layout {
                                let pos = WindowPosition {
                                    workspace_id: ws_id,
                                    column: col,
                                    tile,
                                };
                                new_tracked.insert(w.id, pos);
                            }
                        }
                    }
                }

                for (&id, &pos) in &new_tracked {
                    if self.tracked_window_positions.get(&id) != Some(&pos) {
                        affected_workspaces.push(pos.workspace_id);
                    }
                }
                for (&id, &pos) in &self.tracked_window_positions {
                    if new_tracked.get(&id) != Some(&pos) {
                        affected_workspaces.push(pos.workspace_id);
                    }
                }

                self.tracked_window_positions = new_tracked;
            }

            Event::WindowOpenedOrChanged { window } => {
                let id = window.id;
                let ws_id_opt = window.workspace_id;
                let is_floating = window.is_floating;

                let old_pos = self.tracked_window_positions.get(&id).copied();

                if is_floating {
                    if let Some(pos) = old_pos {
                        self.tracked_window_positions.remove(&id);
                        info!(
                            "window {} became floating, re-evaluating ws {}",
                            id, pos.workspace_id
                        );
                        affected_workspaces.push(pos.workspace_id);
                    }
                } else if let (Some(ws_id), Some((col, tile))) =
                    (ws_id_opt, window.layout.pos_in_scrolling_layout)
                {
                    let new_pos = WindowPosition {
                        workspace_id: ws_id,
                        column: col,
                        tile,
                    };

                    if old_pos != Some(new_pos) {
                        self.tracked_window_positions.insert(id, new_pos);
                        debug!(
                            "window {} position changed to {:?}, re-evaluating",
                            id, new_pos
                        );
                        affected_workspaces.push(ws_id);
                        if let Some(old) = old_pos {
                            if old.workspace_id != ws_id {
                                affected_workspaces.push(old.workspace_id);
                            }
                        }
                    }
                }
            }

            Event::WindowLayoutsChanged { changes } => {
                for (id, layout) in changes {
                    if let Some(pos) = self.tracked_window_positions.get_mut(&id) {
                        if let Some((col, tile)) = layout.pos_in_scrolling_layout {
                            if pos.column != col || pos.tile != tile {
                                debug!(
                                    "window {} layout changed to column {}, tile {}, re-evaluating ws {}",
                                    id, col, tile, pos.workspace_id
                                );
                                pos.column = col;
                                pos.tile = tile;
                                affected_workspaces.push(pos.workspace_id);
                            }
                        }
                    }
                }
            }

            Event::WindowClosed { id } => {
                if let Some(pos) = self.tracked_window_positions.remove(&id) {
                    info!(
                        "window {} closed, re-evaluating ws {}",
                        id, pos.workspace_id
                    );
                    affected_workspaces.push(pos.workspace_id);
                }
            }

            _ => {}
        }

        if !affected_workspaces.is_empty() {
            affected_workspaces.sort_unstable();
            affected_workspaces.dedup();

            let state = self.query_full_state()?;
            let windows_map: HashMap<u64, &Window> =
                state.windows.iter().map(|w| (w.id, w)).collect();

            for ws_id in affected_workspaces {
                if let Err(e) = self.evaluate_workspace(ws_id, &state, &windows_map) {
                    error!("error evaluating workspace {}: {:?}", ws_id, e);
                }
            }
        }

        Ok(())
    }
}
