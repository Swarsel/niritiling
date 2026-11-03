use crate::connection::{NiriConnection, NiriState, WindowPosition};
use anyhow::Result;
use log::{debug, error, info};
use niri_ipc::{Action, Event, SizeChange, Window};
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

const MAXIMIZED_RATIO_THRESHOLD: f64 = 0.9;
const DEBOUNCE_INTERVAL: Duration = Duration::from_millis(200);
const OUTPUT_SETTLE_INTERVAL: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, PartialEq)]
pub struct OutputSnapshot {
    name: String,
    identity: Option<String>,
    width: Option<f64>,
}

fn output_snapshots(state: &NiriState) -> HashMap<u64, OutputSnapshot> {
    state
        .ws_outputs
        .iter()
        .map(|(&ws_id, name)| {
            (
                ws_id,
                OutputSnapshot {
                    name: name.clone(),
                    identity: state.output_identities.get(name).cloned(),
                    width: state.output_widths.get(name).copied(),
                },
            )
        })
        .collect()
}

fn tiled_columns(state: &NiriState, ws_id: u64) -> BTreeMap<usize, Vec<&Window>> {
    let mut columns: BTreeMap<usize, Vec<&Window>> = BTreeMap::new();
    for w in &state.windows {
        if w.workspace_id == Some(ws_id)
            && !w.is_floating
            && let Some((col_idx, _)) = w.layout.pos_in_scrolling_layout
        {
            columns.entry(col_idx).or_default().push(w);
        }
    }
    for tiles in columns.values_mut() {
        tiles.sort_by_key(|w| {
            w.layout
                .pos_in_scrolling_layout
                .map(|(_, tile_idx)| tile_idx)
        });
    }
    columns
}

fn is_maximized(window: &Window, state: &NiriState) -> bool {
    let output_width = window
        .workspace_id
        .and_then(|ws_id| state.ws_outputs.get(&ws_id))
        .and_then(|output_name| state.output_widths.get(output_name))
        .copied()
        .unwrap_or(0.0);
    if output_width <= 0.0 {
        return false;
    }
    let tile_width = window.layout.tile_size.0;
    let ratio = tile_width / output_width;
    debug!(
        "window {} tile_width={:.0} output_width={:.0} ratio={:.2}",
        window.id, tile_width, output_width, ratio
    );
    ratio > MAXIMIZED_RATIO_THRESHOLD
}

fn columns_settled(state: &NiriState, ws_id: u64, output_width: f64, share: f64) -> bool {
    let entries: Vec<(f64, Option<f64>)> = tiled_columns(state, ws_id)
        .into_values()
        .map(|tiles| {
            (
                tiles[0].layout.tile_size.0,
                tiles[0].layout.tile_pos_in_workspace_view.map(|(x, _)| x),
            )
        })
        .collect();
    let gap = entries
        .windows(2)
        .find_map(|pair| Some((pair[1].1? - pair[0].1? - pair[0].0).max(0.0)))
        .unwrap_or(0.0);
    let expected = (output_width - gap) * share / 100.0 - gap;

    entries
        .iter()
        .all(|&(width, _)| (width - expected).abs() <= 20.0)
        && entries.iter().map(|&(width, _)| width).sum::<f64>()
            <= output_width - gap * (entries.len() + 1) as f64 + 2.0
}

pub struct NiriContext {
    pub connection: Box<dyn NiriConnection>,
    pub tracked_window_positions: HashMap<u64, WindowPosition>,
    pub debounced_maximize_state: HashMap<u64, (bool, Instant)>,
    pub tracked_column_widths: HashMap<u64, HashMap<usize, f64>>,
    pub tracked_column_counts: HashMap<u64, usize>,
    pub last_maximize_action_time: HashMap<u64, Instant>,
    pub tracked_outputs: HashMap<u64, OutputSnapshot>,
    pub output_settle_interval: Duration,
    pub resize_columns: bool,
    pub column_targets: HashMap<String, usize>,
    pub default_columns: usize,
}

impl NiriContext {
    pub fn new(
        connection: Box<dyn NiriConnection>,
        resize_columns: bool,
        column_targets: HashMap<String, usize>,
        default_columns: usize,
    ) -> Self {
        Self {
            connection,
            tracked_window_positions: HashMap::new(),
            debounced_maximize_state: HashMap::new(),
            tracked_column_widths: HashMap::new(),
            tracked_column_counts: HashMap::new(),
            last_maximize_action_time: HashMap::new(),
            tracked_outputs: HashMap::new(),
            output_settle_interval: OUTPUT_SETTLE_INTERVAL,
            resize_columns,
            column_targets,
            default_columns,
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

    fn debounce_allows(&mut self, win_id: u64, target_maximized: bool) -> bool {
        let now = Instant::now();
        if let Some(&(prev_target, last_time)) = self.debounced_maximize_state.get(&win_id)
            && prev_target == target_maximized
            && now.duration_since(last_time) < DEBOUNCE_INTERVAL
        {
            debug!(
                "window {}: skipping repeated maximize state change due to debounce",
                win_id
            );
            return false;
        }
        self.debounced_maximize_state
            .insert(win_id, (target_maximized, now));
        true
    }

    fn nudge_viewport_left(&mut self) {
        let focused = self.query_focused_window().ok().flatten();
        let _ = self.send_action(Action::FocusColumnLeft {});
        if let Some(orig_id) = focused {
            let _ = self.send_action(Action::FocusWindow { id: orig_id });
        }
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

        if restore_focus
            && let Some(orig_id) = original_focus
            && orig_id != target_window_id
        {
            debug!("restoring focus to {}", orig_id);
            let _ = self.send_action(Action::FocusWindow { id: orig_id });
        }
        Ok(())
    }

    pub fn evaluate_workspace(&mut self, ws_id: u64, state: &NiriState) -> Result<()> {
        let columns = tiled_columns(state, ws_id);

        if columns.len() == 1 {
            let window = columns.values().next().unwrap()[0];
            if !is_maximized(window, state) && self.debounce_allows(window.id, true) {
                info!(
                    "workspace {}: single column -> maximizing window {}",
                    ws_id, window.id
                );
                self.perform_maximize_action(window.id, true)?;
                self.last_maximize_action_time.insert(ws_id, Instant::now());
            }
            return Ok(());
        }

        let mut original_focus = None;
        let mut did_unmaximize = false;
        for (&col_idx, tiles) in &columns {
            let window = tiles[0];
            if is_maximized(window, state) && self.debounce_allows(window.id, false) {
                if !did_unmaximize {
                    original_focus = self.query_focused_window().ok().flatten();
                }
                info!(
                    "workspace {}: multiple columns -> un-maximizing window {} in column {}",
                    ws_id, window.id, col_idx
                );
                self.perform_maximize_action(window.id, false)?;
                did_unmaximize = true;
            }
        }

        if did_unmaximize {
            self.last_maximize_action_time.insert(ws_id, Instant::now());
            debug!(
                "workspace {}: waiting for layout to settle before viewport nudge",
                ws_id
            );
            std::thread::sleep(Duration::from_millis(50));

            debug!(
                "workspace {}: nudging viewport left (target focus: {:?})",
                ws_id, original_focus
            );
            self.send_action(Action::FocusColumnLeft {})?;
            if let Some(orig_id) = original_focus {
                debug!("workspace {}: restoring focus to {}", ws_id, orig_id);
                let _ = self.send_action(Action::FocusWindow { id: orig_id });
            }
        }
        Ok(())
    }

    fn column_target(&self, ws_id: u64, state: &NiriState) -> Option<usize> {
        if self.column_targets.is_empty() {
            return None;
        }
        let output_name = state.ws_outputs.get(&ws_id)?;
        self.column_targets
            .get(state.output_identities.get(output_name)?)
            .copied()
            .filter(|&t| t > 0)
    }

    fn stack_into_single_column(&mut self, ws_id: u64, state: &NiriState) -> Result<bool> {
        if self.column_target(ws_id, state) != Some(1) {
            return Ok(false);
        }

        let columns = tiled_columns(state, ws_id);
        if columns.len() < 2 {
            return Ok(false);
        }

        let mut stack: Vec<Vec<u64>> = columns
            .into_values()
            .map(|tiles| tiles.iter().map(|w| w.id).collect())
            .collect();

        info!(
            "workspace {}: {} columns on single-column output -> stacking into one column",
            ws_id,
            stack.len()
        );

        while stack.len() > 1 {
            let win_id = stack[1][0];
            self.send_action(Action::ConsumeOrExpelWindowLeft { id: Some(win_id) })?;
            if stack[1].len() == 1 {
                stack.remove(1);
                stack[0].push(win_id);
            } else {
                stack[1].remove(0);
                stack.insert(1, vec![win_id]);
            }
        }

        Ok(true)
    }

    fn wait_for_columns_to_settle(&mut self, ws_id: u64, output_width: f64, share: f64) {
        for _ in 0..24 {
            match self.query_full_state() {
                Ok(state) if columns_settled(&state, ws_id, output_width, share) => return,
                Ok(_) => std::thread::sleep(Duration::from_millis(25)),
                Err(_) => return,
            }
        }
        debug!(
            "workspace {}: columns did not settle within timeout, nudging anyway",
            ws_id
        );
    }

    fn equalize_columns(&mut self, ws_id: u64, state: &NiriState) -> Result<()> {
        match self.column_target(ws_id, state) {
            Some(target) if target > 1 => self.size_columns(ws_id, state, target, true),
            _ => Ok(()),
        }
    }

    fn reset_columns_after_output_change(
        &mut self,
        ws_id: u64,
        state: &NiriState,
        fix_viewport: bool,
    ) -> Result<()> {
        self.tracked_column_counts.remove(&ws_id);
        self.tracked_column_widths.remove(&ws_id);
        let target = self
            .column_target(ws_id, state)
            .unwrap_or(self.default_columns);
        info!(
            "workspace {}: output changed -> resetting columns for target {}",
            ws_id, target
        );
        self.size_columns(ws_id, state, target, fix_viewport)
    }

    pub fn seed_outputs(&mut self, state: &NiriState) {
        self.tracked_outputs = output_snapshots(state);
    }

    fn take_output_changes(&mut self, state: &mut NiriState) -> Result<Vec<u64>> {
        let mut snapshots = output_snapshots(state);
        if !self.changed_outputs(&snapshots).is_empty() {
            info!(
                "output configuration changed, waiting {:?} for it to settle",
                self.output_settle_interval
            );
            std::thread::sleep(self.output_settle_interval);
            *state = self.query_full_state()?;
            snapshots = output_snapshots(state);
        }
        let changed = self.changed_outputs(&snapshots);
        self.tracked_outputs = snapshots;
        Ok(changed)
    }

    fn changed_outputs(&self, snapshots: &HashMap<u64, OutputSnapshot>) -> Vec<u64> {
        snapshots
            .iter()
            .filter(|&(ws_id, snapshot)| {
                self.tracked_outputs
                    .get(ws_id)
                    .is_some_and(|prev| prev != snapshot)
            })
            .map(|(&ws_id, _)| ws_id)
            .collect()
    }

    fn size_columns(
        &mut self,
        ws_id: u64,
        state: &NiriState,
        target: usize,
        fix_viewport: bool,
    ) -> Result<()> {
        let output_name = match state.ws_outputs.get(&ws_id) {
            Some(name) => name,
            None => return Ok(()),
        };

        let column_ids: Vec<u64> = tiled_columns(state, ws_id)
            .into_values()
            .map(|tiles| tiles[0].id)
            .collect();

        let count = column_ids.len();
        let prev = self.tracked_column_counts.insert(ws_id, count);
        if count < 2 || prev == Some(count) {
            return Ok(());
        }

        let share = 100.0 / (count.min(target) as f64);
        info!(
            "workspace {}: {} columns on output {} -> sizing columns to {:.1}%",
            ws_id, count, output_name, share
        );
        for &win_id in &column_ids {
            self.send_action(Action::SetWindowWidth {
                id: Some(win_id),
                change: SizeChange::SetProportion(share),
            })?;
        }

        if !fix_viewport {
            return Ok(());
        }

        match state.output_widths.get(output_name) {
            Some(&width) if width > 0.0 && count <= target => {
                self.wait_for_columns_to_settle(ws_id, width, share)
            }
            _ => std::thread::sleep(Duration::from_millis(50)),
        }

        let focused = self.query_focused_window().ok().flatten();
        if let Some(&first_id) = column_ids.first() {
            self.send_action(Action::FocusWindow { id: first_id })?;
        }
        if let Some(&last_id) = column_ids.last() {
            self.send_action(Action::FocusWindow { id: last_id })?;
        }
        if let Some(orig_id) = focused {
            let _ = self.send_action(Action::FocusWindow { id: orig_id });
        }
        Ok(())
    }

    fn redistribute_on_column_resize(&mut self, ws_id: u64, state: &NiriState) -> Result<()> {
        let output_name = match state.ws_outputs.get(&ws_id) {
            Some(name) => name,
            None => return Ok(()),
        };
        let output_width = match state.output_widths.get(output_name) {
            Some(&w) if w > 0.0 => w,
            _ => return Ok(()),
        };

        let current: Vec<(usize, f64, u64)> = tiled_columns(state, ws_id)
            .into_iter()
            .map(|(col_idx, tiles)| (col_idx, tiles[0].layout.tile_size.0, tiles[0].id))
            .collect();

        if current.len() != 2 {
            self.tracked_column_widths.remove(&ws_id);
            return Ok(());
        }

        let suppressed = self
            .last_maximize_action_time
            .get(&ws_id)
            .is_some_and(|t| t.elapsed() < Duration::from_millis(500));

        let any_maximized = current
            .iter()
            .any(|&(_, width, _)| width / output_width > MAXIMIZED_RATIO_THRESHOLD);

        if !suppressed
            && !any_maximized
            && let Some(prev) = self.tracked_column_widths.get(&ws_id)
            && prev.len() == 2
        {
            let deltas: Vec<f64> = current
                .iter()
                .map(|&(col_idx, width, _)| width - prev.get(&col_idx).copied().unwrap_or(width))
                .collect();
            let changed: Vec<usize> = (0..deltas.len())
                .filter(|&i| deltas[i].abs() > 2.0)
                .collect();

            if let [changed_idx] = changed[..] {
                let (changed_col, changed_width, _) = current[changed_idx];
                let (other_col, other_width, other_id) = current[1 - changed_idx];
                let delta = deltas[changed_idx];
                let delta_px = delta.round() as i32;
                info!(
                    "workspace {}: column {} resized by {}px, adjusting column {} (window {})",
                    ws_id, changed_col, delta_px, other_col, other_id
                );
                self.send_action(Action::SetWindowWidth {
                    id: Some(other_id),
                    change: SizeChange::AdjustFixed(-delta_px),
                })?;

                if other_col < changed_col && delta_px < 0 {
                    std::thread::sleep(Duration::from_millis(50));
                    self.nudge_viewport_left();
                }

                self.tracked_column_widths.insert(
                    ws_id,
                    HashMap::from([
                        (changed_col, changed_width),
                        (other_col, other_width - delta),
                    ]),
                );
                return Ok(());
            }
        }

        self.tracked_column_widths.insert(
            ws_id,
            current
                .into_iter()
                .map(|(col_idx, width, _)| (col_idx, width))
                .collect(),
        );

        Ok(())
    }

    pub fn handle_event(&mut self, event: Event) -> Result<()> {
        let mut affected_workspaces = Vec::new();
        let mut closed_positions: Vec<WindowPosition> = Vec::new();
        let workspaces_changed = matches!(event, Event::WorkspacesChanged { .. });

        match event {
            Event::WindowsChanged { windows } => {
                debug!("full windows change event received");
                let mut new_tracked = HashMap::with_capacity(windows.len());

                for w in windows {
                    if !w.is_floating
                        && let Some(ws_id) = w.workspace_id
                    {
                        let (col, tile) = w.layout.pos_in_scrolling_layout.unzip();
                        new_tracked.insert(
                            w.id,
                            WindowPosition {
                                workspace_id: ws_id,
                                column: col,
                                tile,
                            },
                        );
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

                self.debounced_maximize_state
                    .retain(|id, _| new_tracked.contains_key(id));
                self.tracked_window_positions = new_tracked;
            }

            Event::WindowOpenedOrChanged { window } => {
                let id = window.id;
                let old_pos = self.tracked_window_positions.get(&id).copied();

                if window.is_floating {
                    if let Some(pos) = old_pos {
                        self.tracked_window_positions.remove(&id);
                        info!(
                            "window {} became floating, re-evaluating ws {}",
                            id, pos.workspace_id
                        );
                        affected_workspaces.push(pos.workspace_id);
                    }
                } else if let Some(ws_id) = window.workspace_id {
                    let (col, tile) = window.layout.pos_in_scrolling_layout.unzip();
                    let new_pos = WindowPosition {
                        workspace_id: ws_id,
                        column: col,
                        tile,
                    };

                    self.tracked_window_positions.insert(id, new_pos);
                    debug!(
                        "window {} position updated to {:?}, re-evaluating",
                        id, new_pos
                    );
                    affected_workspaces.push(ws_id);
                    if let Some(old) = old_pos
                        && old.workspace_id != ws_id
                    {
                        affected_workspaces.push(old.workspace_id);
                    }
                }
            }

            Event::WindowLayoutsChanged { changes } => {
                for (id, layout) in changes {
                    if let Some(pos) = self.tracked_window_positions.get_mut(&id) {
                        let (col, tile) = layout.pos_in_scrolling_layout.unzip();
                        debug!(
                            "window {} layout updated to column {:?}, tile {:?}, re-evaluating ws {}",
                            id, col, tile, pos.workspace_id
                        );
                        pos.column = col;
                        pos.tile = tile;
                        affected_workspaces.push(pos.workspace_id);
                    }
                }
            }

            Event::WindowClosed { id } => {
                self.debounced_maximize_state.remove(&id);
                if let Some(pos) = self.tracked_window_positions.remove(&id) {
                    info!(
                        "window {} closed, re-evaluating ws {}",
                        id, pos.workspace_id
                    );
                    affected_workspaces.push(pos.workspace_id);
                    closed_positions.push(pos);
                }
            }

            _ => {}
        }

        if !affected_workspaces.is_empty() || workspaces_changed {
            std::thread::sleep(Duration::from_millis(20));

            let mut state = self.query_full_state()?;

            let output_changed = self.take_output_changes(&mut state)?;
            affected_workspaces.extend(&output_changed);
            affected_workspaces.sort_unstable();
            affected_workspaces.dedup();

            let mut stacked = false;
            for &ws_id in &affected_workspaces {
                match self.stack_into_single_column(ws_id, &state) {
                    Ok(acted) => stacked |= acted,
                    Err(e) => error!("error stacking columns for workspace {}: {:?}", ws_id, e),
                }
            }
            if stacked {
                std::thread::sleep(Duration::from_millis(50));
                state = self.query_full_state()?;
            }

            for ws_id in affected_workspaces {
                let moved = output_changed.contains(&ws_id);
                let may_focus = !moved || state.active_workspaces.contains(&ws_id);
                if may_focus && let Err(e) = self.evaluate_workspace(ws_id, &state) {
                    error!("error evaluating workspace {}: {:?}", ws_id, e);
                }
                let sized = if moved {
                    self.reset_columns_after_output_change(ws_id, &state, may_focus)
                } else {
                    self.equalize_columns(ws_id, &state)
                };
                if let Err(e) = sized {
                    error!("error equalizing columns for workspace {}: {:?}", ws_id, e);
                }
                if self.resize_columns
                    && !moved
                    && let Err(e) = self.redistribute_on_column_resize(ws_id, &state)
                {
                    error!(
                        "error redistributing columns for workspace {}: {:?}",
                        ws_id, e
                    );
                }
            }

            for closed_pos in &closed_positions {
                if !self
                    .tracked_window_positions
                    .values()
                    .any(|p| p.workspace_id == closed_pos.workspace_id)
                {
                    self.last_maximize_action_time
                        .remove(&closed_pos.workspace_id);
                    self.tracked_column_counts.remove(&closed_pos.workspace_id);
                    self.tracked_column_widths.remove(&closed_pos.workspace_id);
                    continue;
                }
                let min_remaining_col = self
                    .tracked_window_positions
                    .values()
                    .filter(|p| p.workspace_id == closed_pos.workspace_id)
                    .filter_map(|p| p.column)
                    .min();
                if let (Some(closed_col), Some(min_col)) = (closed_pos.column, min_remaining_col)
                    && closed_col > min_col
                {
                    debug!(
                        "closed window column {} had columns to the left, nudging viewport left",
                        closed_col
                    );
                    self.nudge_viewport_left();
                }
            }
        }

        Ok(())
    }
}
