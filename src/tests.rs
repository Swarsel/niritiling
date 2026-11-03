use super::connection::{NiriConnection, NiriState, WindowPosition};
use super::manager::NiriContext;
use anyhow::Result;
use niri_ipc::{Action, Event, SizeChange, Window};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
pub struct MockState {
    pub actions: Vec<Action>,
    pub focused_window: Option<u64>,
    pub state: NiriState,
}

pub struct MockConnection {
    pub shared: Arc<Mutex<MockState>>,
}

impl NiriConnection for MockConnection {
    fn send_action(&mut self, action: Action) -> Result<()> {
        let mut shared = self.shared.lock().unwrap();
        if let Action::SetWindowWidth {
            id: Some(id),
            change: SizeChange::SetProportion(p),
        } = action
        {
            let width = shared
                .state
                .windows
                .iter()
                .find(|w| w.id == id)
                .and_then(|w| w.workspace_id)
                .and_then(|ws| shared.state.ws_outputs.get(&ws))
                .and_then(|out| shared.state.output_widths.get(out))
                .map(|&ow| ow * p / 100.0);
            if let (Some(width), Some(w)) =
                (width, shared.state.windows.iter_mut().find(|w| w.id == id))
            {
                w.layout.tile_size.0 = width;
            }
        }
        shared.actions.push(action);
        Ok(())
    }
    fn query_focused_window(&mut self) -> Result<Option<u64>> {
        Ok(self.shared.lock().unwrap().focused_window)
    }
    fn query_full_state(&mut self) -> Result<NiriState> {
        Ok(self.shared.lock().unwrap().state.clone())
    }
}

pub struct SettlingMockConnection {
    pub shared: Arc<Mutex<MockState>>,
    pub queries_until_settle: Arc<Mutex<usize>>,
    pub settled_width: f64,
}

impl NiriConnection for SettlingMockConnection {
    fn send_action(&mut self, action: Action) -> Result<()> {
        self.shared.lock().unwrap().actions.push(action);
        Ok(())
    }
    fn query_focused_window(&mut self) -> Result<Option<u64>> {
        Ok(self.shared.lock().unwrap().focused_window)
    }
    fn query_full_state(&mut self) -> Result<NiriState> {
        let mut counter = self.queries_until_settle.lock().unwrap();
        if *counter > 0 {
            *counter -= 1;
            if *counter == 0 {
                for w in &mut self.shared.lock().unwrap().state.windows {
                    w.layout.tile_size.0 = self.settled_width;
                }
            }
        }
        Ok(self.shared.lock().unwrap().state.clone())
    }
}

fn create_mock_window_at(
    id: u64,
    ws_id: u64,
    col: usize,
    tile: usize,
    width: f64,
    tile_x: f64,
) -> Window {
    use serde_json::json;
    let w_int = width as i32;
    let v = json!({
        "id": id,
        "title": "test",
        "app_id": "test",
        "workspace_id": ws_id,
        "is_focused": false,
        "is_floating": false,
        "pid": 1234,
        "is_urgent": false,
        "layout": {
            "window_size": [w_int, 0],
            "tile_pos_in_workspace_view": [tile_x, 0],
            "window_offset_in_tile": [0, 0],
            "tile_size": [w_int, 0],
            "pos_in_scrolling_layout": [col, tile]
        }
    });
    serde_json::from_value(v).expect("failed to deserialize mock window")
}

fn create_mock_window(id: u64, ws_id: u64, col: usize, tile: usize, width: f64) -> Window {
    create_mock_window_at(id, ws_id, col, tile, width, 0.0)
}

fn create_mock_fullscreen_window(id: u64, ws_id: u64, width: f64) -> Window {
    use serde_json::json;
    let w_int = width as i32;
    let v = json!({
        "id": id,
        "title": "test",
        "app_id": "test",
        "workspace_id": ws_id,
        "is_focused": false,
        "is_floating": false,
        "pid": 1234,
        "is_urgent": false,
        "layout": {
            "window_size": [w_int, 0],
            "tile_pos_in_workspace_view": [0, 0],
            "window_offset_in_tile": [0, 0],
            "tile_size": [w_int, 0],
            "pos_in_scrolling_layout": null
        }
    });
    serde_json::from_value(v).expect("failed to deserialize mock window")
}

fn track(ctx: &mut NiriContext, positions: &[(u64, usize, usize)]) {
    for &(id, col, tile) in positions {
        ctx.tracked_window_positions.insert(
            id,
            WindowPosition {
                workspace_id: 1,
                column: Some(col),
                tile: Some(tile),
            },
        );
    }
}

fn setup_test(windows: Vec<Window>) -> (NiriContext, Arc<Mutex<MockState>>) {
    setup_test_with_resize(windows, false)
}

fn setup_test_with_resize(
    windows: Vec<Window>,
    resize_columns: bool,
) -> (NiriContext, Arc<Mutex<MockState>>) {
    let output_name = "eDP-1".to_string();
    let shared = Arc::new(Mutex::new(MockState {
        actions: Vec::new(),
        focused_window: None,
        state: NiriState {
            windows,
            output_widths: HashMap::from([(output_name.clone(), 1000.0)]),
            output_identities: HashMap::new(),
            ws_outputs: HashMap::from([(1, output_name)]),
            active_workspaces: HashSet::new(),
        },
    }));

    let conn = Box::new(MockConnection {
        shared: shared.clone(),
    });
    (
        NiriContext::new(conn, resize_columns, HashMap::new(), 2),
        shared,
    )
}

const WIDE_IDENTITY: &str = "LG Electronics LG ULTRAFINE 510NTEPPU036";

fn wide_mock_state(windows: Vec<Window>, output_width: f64) -> Arc<Mutex<MockState>> {
    let output_name = "DP-1".to_string();
    Arc::new(Mutex::new(MockState {
        actions: Vec::new(),
        focused_window: None,
        state: NiriState {
            windows,
            output_widths: HashMap::from([(output_name.clone(), output_width)]),
            output_identities: HashMap::from([(output_name.clone(), WIDE_IDENTITY.to_string())]),
            ws_outputs: HashMap::from([(1, output_name)]),
            active_workspaces: HashSet::new(),
        },
    }))
}

fn setup_test_wide(
    windows: Vec<Window>,
    output_width: f64,
    target_identity: &str,
    columns: usize,
) -> (NiriContext, Arc<Mutex<MockState>>) {
    let shared = wide_mock_state(windows, output_width);
    let column_targets = HashMap::from([(target_identity.to_string(), columns)]);
    let conn = Box::new(MockConnection {
        shared: shared.clone(),
    });
    (NiriContext::new(conn, false, column_targets, 2), shared)
}

fn setup_test_settling(
    windows: Vec<Window>,
    output_width: f64,
    queries_until_settle: usize,
    settled_width: f64,
) -> (NiriContext, Arc<Mutex<MockState>>, Arc<Mutex<usize>>) {
    let shared = wide_mock_state(windows, output_width);
    let counter = Arc::new(Mutex::new(queries_until_settle));
    let ctx = NiriContext::new(
        Box::new(SettlingMockConnection {
            shared: shared.clone(),
            queries_until_settle: counter.clone(),
            settled_width,
        }),
        false,
        HashMap::from([(WIDE_IDENTITY.to_string(), 3)]),
        2,
    );
    (ctx, shared, counter)
}

const LAPTOP_IDENTITY: &str = "BOE NE135A1M-NY1 Unknown";

fn setup_test_two_outputs(
    windows: Vec<Window>,
    column_targets: &[(&str, usize)],
) -> (NiriContext, Arc<Mutex<MockState>>) {
    let shared = Arc::new(Mutex::new(MockState {
        actions: Vec::new(),
        focused_window: None,
        state: NiriState {
            windows,
            output_widths: HashMap::from([
                ("eDP-1".to_string(), 1000.0),
                ("DP-1".to_string(), 5120.0),
            ]),
            output_identities: HashMap::from([
                ("eDP-1".to_string(), LAPTOP_IDENTITY.to_string()),
                ("DP-1".to_string(), WIDE_IDENTITY.to_string()),
            ]),
            ws_outputs: HashMap::from([(1, "DP-1".to_string())]),
            active_workspaces: HashSet::from([1]),
        },
    }));
    let column_targets = column_targets
        .iter()
        .map(|&(identity, n)| (identity.to_string(), n))
        .collect();
    let mut ctx = NiriContext::new(
        Box::new(MockConnection {
            shared: shared.clone(),
        }),
        false,
        column_targets,
        2,
    );
    ctx.output_settle_interval = Duration::ZERO;
    (ctx, shared)
}

fn initial_sync(ctx: &mut NiriContext, shared: &Arc<Mutex<MockState>>) {
    let windows = shared.lock().unwrap().state.windows.clone();
    ctx.handle_event(Event::WindowsChanged { windows }).unwrap();
}

fn move_workspace(ctx: &mut NiriContext, shared: &Arc<Mutex<MockState>>, output: &str) {
    initial_sync(ctx, shared);
    let mut shared = shared.lock().unwrap();
    shared.actions.clear();
    shared.state.ws_outputs.insert(1, output.to_string());
    drop(shared);
    ctx.handle_event(Event::WorkspacesChanged {
        workspaces: Vec::new(),
    })
    .unwrap();
}

fn three_wide_columns() -> Vec<Window> {
    vec![
        create_mock_window(100, 1, 0, 0, 1706.0),
        create_mock_window(200, 1, 1, 0, 1706.0),
        create_mock_window(300, 1, 2, 0, 1706.0),
    ]
}

fn is_focus_action(action: &Action) -> bool {
    matches!(
        action,
        Action::FocusWindow { .. } | Action::FocusColumnLeft {} | Action::MaximizeColumn {}
    )
}

fn consume_left_ids(actions: &[Action]) -> Vec<u64> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::ConsumeOrExpelWindowLeft { id: Some(id) } => Some(*id),
            _ => None,
        })
        .collect()
}

fn proportion_actions(actions: &[Action]) -> Vec<(u64, f64)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::SetWindowWidth {
                id: Some(id),
                change: SizeChange::SetProportion(p),
            } => Some((*id, *p)),
            _ => None,
        })
        .collect()
}

#[test]
fn test_opened_on_empty_maximizes() {
    let (mut ctx, shared) = setup_test(Vec::new());
    let win = create_mock_window(100, 1, 0, 0, 500.0);

    shared.lock().unwrap().state.windows.push(win.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged { window: win })
        .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {}))
    );
}

#[test]
fn test_two_columns_unmaximize() {
    let win1 = create_mock_window(100, 1, 0, 0, 1000.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone()]);

    track(&mut ctx, &[(100, 0, 0)]);

    let win2 = create_mock_window(101, 1, 1, 0, 1000.0);
    shared.lock().unwrap().state.windows.push(win2.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged { window: win2 })
        .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {})),
        "MaximizeColumn should be sent to un-maximize"
    );
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::FocusColumnLeft {})),
        "FocusColumnLeft should be sent after un-maximizing"
    );
}

#[test]
fn test_close_one_of_two_columns_maximizes_remaining() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    track(&mut ctx, &[(100, 0, 0), (101, 1, 0)]);

    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    ctx.handle_event(Event::WindowClosed { id: 101 }).unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {}))
    );
}

#[test]
fn test_close_second_to_last_on_three_columns_nudges_viewport() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let win3 = create_mock_window(102, 1, 2, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone(), win3.clone()]);

    track(&mut ctx, &[(100, 0, 0), (101, 1, 0), (102, 2, 0)]);

    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    ctx.handle_event(Event::WindowClosed { id: 101 }).unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::FocusColumnLeft {})),
        "FocusColumnLeft should be sent to nudge viewport left after closing middle column"
    );
}

#[test]
fn test_drag_into_column_maximizes_if_one_left() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    track(&mut ctx, &[(100, 0, 0), (101, 1, 0)]);

    let win2_new = create_mock_window(101, 1, 0, 1, 500.0);
    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    shared.lock().unwrap().state.windows.push(win2_new.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged { window: win2_new })
        .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {}))
    );
}

#[test]
fn test_drag_out_of_column_nudges_and_unmaximizes() {
    let win1 = create_mock_window(100, 1, 0, 0, 1000.0);
    let win2 = create_mock_window(101, 1, 0, 1, 1000.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    track(&mut ctx, &[(100, 0, 0), (101, 0, 1)]);

    let win2_new = create_mock_window(101, 1, 1, 0, 1000.0);
    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    shared.lock().unwrap().state.windows.push(win2_new.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged { window: win2_new })
        .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {}))
    );
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::FocusColumnLeft {}))
    );
}

#[test]
fn test_repro_fullscreen_move_bug() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2_tiled = create_mock_window(101, 1, 1, 0, 500.0);

    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2_tiled.clone()]);

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1.clone(), win2_tiled.clone()],
    })
    .unwrap();

    let win2_fs = create_mock_fullscreen_window(101, 1, 1000.0);
    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    shared.lock().unwrap().state.windows.push(win2_fs.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged { window: win2_fs })
        .unwrap();

    shared.lock().unwrap().actions.clear();
    ctx.debounced_maximize_state.clear();

    let win2_fs_ws2 = create_mock_fullscreen_window(101, 2, 1000.0);
    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    shared
        .lock()
        .unwrap()
        .state
        .windows
        .push(win2_fs_ws2.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win2_fs_ws2,
    })
    .unwrap();

    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    ctx.handle_event(Event::WindowClosed { id: 101 }).unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {})),
        "MaximizeColumn should have been called for win1 in WS 1"
    );
}

#[test]
fn test_repro_fullscreen_started_fs_bug() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2_fs_ws1 = create_mock_fullscreen_window(101, 1, 1000.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2_fs_ws1.clone()]);

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1.clone(), win2_fs_ws1.clone()],
    })
    .unwrap();

    let win2_fs_ws2 = create_mock_fullscreen_window(101, 2, 1000.0);
    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    shared
        .lock()
        .unwrap()
        .state
        .windows
        .push(win2_fs_ws2.clone());

    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win2_fs_ws2,
    })
    .unwrap();

    {
        let actions = &shared.lock().unwrap().actions;
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::MaximizeColumn {})),
            "WS 1 should have been re-evaluated after win2 moved away"
        );
    }

    shared.lock().unwrap().actions.clear();

    shared.lock().unwrap().state.windows.retain(|w| w.id != 101);
    ctx.handle_event(Event::WindowClosed { id: 101 }).unwrap();
}

#[test]
fn test_drag_left_into_right_column_maximizes() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    track(&mut ctx, &[(100, 0, 0), (101, 1, 0)]);

    let win1_stacked = create_mock_window(100, 1, 0, 0, 500.0);
    let win2_stacked = create_mock_window(101, 1, 0, 1, 500.0);
    shared.lock().unwrap().state.windows = vec![win1_stacked.clone(), win2_stacked.clone()];

    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win1_stacked,
    })
    .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {})),
        "Should maximize the single remaining column after dragging left into right"
    );
}

#[test]
fn test_drag_left_into_right_column_via_layout_change() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    track(&mut ctx, &[(100, 0, 0), (101, 1, 0)]);

    let win1_stacked = create_mock_window(100, 1, 0, 0, 500.0);
    let win2_stacked = create_mock_window(101, 1, 0, 1, 500.0);
    shared.lock().unwrap().state.windows = vec![win1_stacked.clone(), win2_stacked.clone()];

    ctx.handle_event(Event::WindowLayoutsChanged {
        changes: vec![(100, win1_stacked.layout)],
    })
    .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {})),
        "Should maximize via layout change even when position appears unchanged"
    );
}

#[test]
fn test_stale_state_does_not_send_focus_column_left() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    track(&mut ctx, &[(100, 0, 0), (101, 1, 0)]);

    let win1_moved = create_mock_window(100, 1, 0, 0, 500.0);
    ctx.handle_event(Event::WindowOpenedOrChanged { window: win1_moved })
        .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::FocusColumnLeft {})),
        "FocusColumnLeft should NOT be sent when stale state shows 2 non-maximized columns"
    );
}

#[test]
fn test_second_event_after_stale_state_maximizes() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone()]);

    track(&mut ctx, &[(100, 0, 0), (101, 1, 0)]);

    let win1_moved = create_mock_window(100, 1, 0, 0, 500.0);
    ctx.handle_event(Event::WindowOpenedOrChanged { window: win1_moved })
        .unwrap();

    shared.lock().unwrap().actions.clear();

    let win1_stacked = create_mock_window(100, 1, 0, 0, 500.0);
    let win2_stacked = create_mock_window(101, 1, 0, 1, 500.0);
    shared.lock().unwrap().state.windows = vec![win1_stacked.clone(), win2_stacked.clone()];

    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win2_stacked,
    })
    .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::MaximizeColumn {})),
        "Should maximize when second event sees settled 1-column state"
    );
}

#[test]
fn test_close_rightmost_of_three_columns_nudges_viewport_left() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(101, 1, 1, 0, 500.0);
    let win3 = create_mock_window(102, 1, 2, 0, 500.0);
    let (mut ctx, shared) = setup_test(vec![win1.clone(), win2.clone(), win3.clone()]);

    track(&mut ctx, &[(100, 0, 0), (101, 1, 0), (102, 2, 0)]);

    shared.lock().unwrap().state.windows.retain(|w| w.id != 102);
    ctx.handle_event(Event::WindowClosed { id: 102 }).unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::FocusColumnLeft {})),
        "FocusColumnLeft should be sent to nudge viewport left after closing rightmost column"
    );
}

#[test]
fn test_resize_column_redistributes_two_columns() {
    let win1 = create_mock_window_at(100, 1, 0, 0, 500.0, 0.0);
    let win2 = create_mock_window_at(101, 1, 1, 0, 500.0, 500.0);
    let (mut ctx, shared) = setup_test_with_resize(vec![win1.clone(), win2.clone()], true);

    track(&mut ctx, &[(100, 0, 0), (101, 1, 0)]);

    ctx.tracked_column_widths
        .insert(1, HashMap::from([(0, 500.0), (1, 500.0)]));

    let win1_resized = create_mock_window_at(100, 1, 0, 0, 600.0, 0.0);
    let win2_pushed = create_mock_window_at(101, 1, 1, 0, 500.0, 600.0);
    shared.lock().unwrap().state.windows = vec![win1_resized.clone(), win2_pushed];

    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win1_resized,
    })
    .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        actions.iter().any(|a| matches!(
            a,
            Action::SetWindowWidth {
                id: Some(101),
                change: SizeChange::AdjustFixed(-100),
            }
        )),
        "SetWindowWidth should shrink the other column by -100px: actions={:?}",
        actions
    );
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::SetWindowWidth { id: Some(100), .. })),
        "The resized column itself should NOT get a SetWindowWidth"
    );
}

#[test]
fn test_resize_does_not_cascade() {
    let win1 = create_mock_window_at(100, 1, 0, 0, 500.0, 0.0);
    let win2 = create_mock_window_at(101, 1, 1, 0, 500.0, 500.0);
    let (mut ctx, shared) = setup_test_with_resize(vec![win1.clone(), win2.clone()], true);

    track(&mut ctx, &[(100, 0, 0), (101, 1, 0)]);

    ctx.tracked_column_widths
        .insert(1, HashMap::from([(0, 500.0), (1, 500.0)]));

    let win1_resized = create_mock_window_at(100, 1, 0, 0, 600.0, 0.0);
    let win2_same = create_mock_window_at(101, 1, 1, 0, 500.0, 600.0);
    shared.lock().unwrap().state.windows = vec![win1_resized.clone(), win2_same];
    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win1_resized,
    })
    .unwrap();

    shared.lock().unwrap().actions.clear();

    let win1_final = create_mock_window_at(100, 1, 0, 0, 600.0, 0.0);
    let win2_final = create_mock_window_at(101, 1, 1, 0, 400.0, 600.0);
    shared.lock().unwrap().state.windows = vec![win1_final.clone(), win2_final];
    ctx.handle_event(Event::WindowOpenedOrChanged { window: win1_final })
        .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::SetWindowWidth { .. })),
        "No further resize should happen after our adjustment settles: actions={:?}",
        actions
    );
}

#[test]
fn test_resize_not_triggered_with_three_columns() {
    let win1 = create_mock_window_at(100, 1, 0, 0, 333.0, 0.0);
    let win2 = create_mock_window_at(101, 1, 1, 0, 333.0, 333.0);
    let win3 = create_mock_window_at(102, 1, 2, 0, 333.0, 666.0);
    let (mut ctx, shared) =
        setup_test_with_resize(vec![win1.clone(), win2.clone(), win3.clone()], true);

    track(&mut ctx, &[(100, 0, 0), (101, 1, 0), (102, 2, 0)]);

    ctx.tracked_column_widths
        .insert(1, HashMap::from([(0, 333.0), (1, 333.0), (2, 333.0)]));

    let win1_resized = create_mock_window_at(100, 1, 0, 0, 433.0, 0.0);
    shared.lock().unwrap().state.windows = vec![win1_resized.clone(), win2.clone(), win3.clone()];

    ctx.handle_event(Event::WindowOpenedOrChanged {
        window: win1_resized,
    })
    .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, Action::SetWindowWidth { .. })),
        "Resize redistribution should NOT happen with 3 columns: actions={:?}",
        actions
    );
}

#[test]
fn test_wide_output_equalizes_to_thirds() {
    let win1 = create_mock_window(100, 1, 1, 1, 2560.0);
    let win2 = create_mock_window(200, 1, 2, 1, 1280.0);
    let win3 = create_mock_window(300, 1, 3, 1, 1280.0);
    let (mut ctx, shared) = setup_test_wide(
        vec![win1.clone(), win2.clone(), win3.clone()],
        5120.0,
        WIDE_IDENTITY,
        3,
    );

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1, win2, win3],
    })
    .unwrap();

    let props = proportion_actions(&shared.lock().unwrap().actions);
    assert_eq!(
        props.len(),
        3,
        "expected all three columns resized: {:?}",
        props
    );
    for (_, p) in &props {
        assert!((p - 100.0 / 3.0).abs() < 0.01, "expected thirds, got {}", p);
    }
}

#[test]
fn test_wide_output_two_columns_get_halves() {
    let win1 = create_mock_window(100, 1, 1, 1, 2560.0);
    let win2 = create_mock_window(200, 1, 2, 1, 1280.0);
    let (mut ctx, shared) =
        setup_test_wide(vec![win1.clone(), win2.clone()], 5120.0, WIDE_IDENTITY, 3);

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1, win2],
    })
    .unwrap();

    let props = proportion_actions(&shared.lock().unwrap().actions);
    assert_eq!(props.len(), 2, "expected both columns resized: {:?}", props);
    for (_, p) in &props {
        assert!((p - 50.0).abs() < 0.01, "expected halves, got {}", p);
    }
}

#[test]
fn test_wide_equalize_only_on_column_count_change() {
    let win1 = create_mock_window(100, 1, 1, 1, 2560.0);
    let win2 = create_mock_window(200, 1, 2, 1, 1280.0);
    let win3 = create_mock_window(300, 1, 3, 1, 1280.0);
    let (mut ctx, shared) = setup_test_wide(
        vec![win1.clone(), win2.clone(), win3.clone()],
        5120.0,
        WIDE_IDENTITY,
        3,
    );

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1.clone(), win2.clone(), win3.clone()],
    })
    .unwrap();
    shared.lock().unwrap().actions.clear();

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1, win2, win3],
    })
    .unwrap();

    let props = proportion_actions(&shared.lock().unwrap().actions);
    assert!(
        props.is_empty(),
        "unchanged column count must not re-equalize: {:?}",
        props
    );
}

#[test]
fn test_non_matching_identity_never_equalizes() {
    let win1 = create_mock_window(100, 1, 0, 0, 500.0);
    let win2 = create_mock_window(200, 1, 1, 0, 300.0);
    let win3 = create_mock_window(300, 1, 2, 0, 200.0);
    let (mut ctx, shared) = setup_test_wide(
        vec![win1.clone(), win2.clone(), win3.clone()],
        5120.0,
        "HP Inc. HP Z32 CN41212T55",
        3,
    );

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1, win2, win3],
    })
    .unwrap();

    let props = proportion_actions(&shared.lock().unwrap().actions);
    assert!(
        props.is_empty(),
        "output with non-matching identity must not be equalized: {:?}",
        props
    );
}

#[test]
fn test_wide_output_above_target_keeps_target_share() {
    let win1 = create_mock_window(100, 1, 1, 1, 1280.0);
    let win2 = create_mock_window(200, 1, 2, 1, 1280.0);
    let win3 = create_mock_window(300, 1, 3, 1, 1280.0);
    let win4 = create_mock_window(400, 1, 4, 1, 1280.0);
    let (mut ctx, shared) = setup_test_wide(
        vec![win1.clone(), win2.clone(), win3.clone(), win4.clone()],
        5120.0,
        WIDE_IDENTITY,
        3,
    );

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1, win2, win3, win4],
    })
    .unwrap();

    let props = proportion_actions(&shared.lock().unwrap().actions);
    assert_eq!(props.len(), 4, "expected four columns resized: {:?}", props);
    for (_, p) in &props {
        assert!((p - 100.0 / 3.0).abs() < 0.01, "expected thirds, got {}", p);
    }
}

#[test]
fn test_single_column_output_stacks_second_column() {
    let win1 = create_mock_window(100, 1, 0, 0, 2160.0);
    let win2 = create_mock_window(200, 1, 1, 0, 2160.0);
    let (mut ctx, shared) =
        setup_test_wide(vec![win1.clone(), win2.clone()], 2160.0, WIDE_IDENTITY, 1);

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1, win2],
    })
    .unwrap();

    assert_eq!(
        consume_left_ids(&shared.lock().unwrap().actions),
        vec![200],
        "the newly opened column should be consumed into the first one"
    );
}

#[test]
fn test_single_column_output_stacks_multi_tile_column() {
    let win1 = create_mock_window(100, 1, 0, 0, 2160.0);
    let win2 = create_mock_window(200, 1, 1, 0, 2160.0);
    let win3 = create_mock_window(300, 1, 1, 1, 2160.0);
    let (mut ctx, shared) = setup_test_wide(
        vec![win1.clone(), win2.clone(), win3.clone()],
        2160.0,
        WIDE_IDENTITY,
        1,
    );

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1, win2, win3],
    })
    .unwrap();

    assert_eq!(
        consume_left_ids(&shared.lock().unwrap().actions),
        vec![200, 200, 300],
        "a stacked column must be expelled tile by tile before merging left"
    );
}

#[test]
fn test_single_column_output_leaves_stack_alone() {
    let win1 = create_mock_window(100, 1, 0, 0, 2160.0);
    let win2 = create_mock_window(200, 1, 0, 1, 2160.0);
    let (mut ctx, shared) =
        setup_test_wide(vec![win1.clone(), win2.clone()], 2160.0, WIDE_IDENTITY, 1);

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1, win2],
    })
    .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        consume_left_ids(actions).is_empty(),
        "windows already in one column must not be touched: {:?}",
        actions
    );
    assert!(
        proportion_actions(actions).is_empty(),
        "single-column outputs must not be equalized: {:?}",
        actions
    );
}

#[test]
fn test_single_column_output_never_equalizes() {
    let win1 = create_mock_window(100, 1, 0, 0, 1080.0);
    let win2 = create_mock_window(200, 1, 1, 0, 1080.0);
    let (mut ctx, shared) =
        setup_test_wide(vec![win1.clone(), win2.clone()], 2160.0, WIDE_IDENTITY, 1);

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1, win2],
    })
    .unwrap();

    let props = proportion_actions(&shared.lock().unwrap().actions);
    assert!(
        props.is_empty(),
        "stacking owns single-column outputs, widths must be left alone: {:?}",
        props
    );
}

#[test]
fn test_multi_column_target_never_stacks() {
    let win1 = create_mock_window(100, 1, 0, 0, 2560.0);
    let win2 = create_mock_window(200, 1, 1, 0, 2560.0);
    let (mut ctx, shared) =
        setup_test_wide(vec![win1.clone(), win2.clone()], 5120.0, WIDE_IDENTITY, 3);

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1, win2],
    })
    .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        consume_left_ids(actions).is_empty(),
        "outputs with room for several columns must not stack: {:?}",
        actions
    );
}

#[test]
fn test_equalize_nudges_only_after_columns_settle() {
    let win1 = create_mock_window(100, 1, 1, 0, 2560.0);
    let win2 = create_mock_window(200, 1, 2, 0, 2560.0);
    let win3 = create_mock_window(300, 1, 3, 0, 2560.0);
    let (mut ctx, shared, queries_until_settle) = setup_test_settling(
        vec![win1.clone(), win2.clone(), win3.clone()],
        5120.0,
        3,
        1706.0,
    );

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1, win2, win3],
    })
    .unwrap();

    assert_eq!(
        *queries_until_settle.lock().unwrap(),
        0,
        "equalize must poll state until the columns fit before nudging the viewport"
    );

    let actions = &shared.lock().unwrap().actions;
    let last_resize = actions
        .iter()
        .rposition(|a| matches!(a, Action::SetWindowWidth { .. }))
        .expect("columns must be resized");
    let first_nudge = actions
        .iter()
        .position(|a| matches!(a, Action::FocusWindow { id: 100 }))
        .expect("viewport must be nudged to the first column");
    let last_nudge = actions
        .iter()
        .position(|a| matches!(a, Action::FocusWindow { id: 300 }))
        .expect("viewport must be nudged to the last column");
    assert!(
        first_nudge > last_resize && last_nudge > first_nudge,
        "nudges must come after the resizes, first column before last: {:?}",
        actions
    );
}

#[test]
fn test_close_nudges_only_after_remaining_columns_settle() {
    let win1 = create_mock_window(100, 1, 1, 0, 1706.0);
    let win2 = create_mock_window(200, 1, 2, 0, 1706.0);
    let (mut ctx, shared, queries_until_settle) =
        setup_test_settling(vec![win1, win2], 5120.0, 3, 2560.0);

    track(&mut ctx, &[(100, 1, 0), (200, 2, 0), (300, 3, 0)]);
    ctx.tracked_column_counts.insert(1, 3);

    ctx.handle_event(Event::WindowClosed { id: 300 }).unwrap();

    assert_eq!(
        *queries_until_settle.lock().unwrap(),
        0,
        "equalize must poll state until the remaining columns grew to halves before nudging"
    );

    let actions = &shared.lock().unwrap().actions;
    let last_resize = actions
        .iter()
        .rposition(|a| matches!(a, Action::SetWindowWidth { .. }))
        .expect("remaining columns must be resized");
    let first_nudge = actions
        .iter()
        .position(|a| matches!(a, Action::FocusWindow { id: 100 }))
        .expect("viewport must be nudged to the first column");
    let last_nudge = actions
        .iter()
        .position(|a| matches!(a, Action::FocusWindow { id: 200 }))
        .expect("viewport must be nudged to the last column");
    assert!(
        first_nudge > last_resize && last_nudge > first_nudge,
        "nudges must come after the resizes, first column before last: {:?}",
        actions
    );
}

#[test]
fn test_single_column_output_ignores_floating_windows() {
    let win1 = create_mock_window(100, 1, 0, 0, 2160.0);
    let mut floating = create_mock_window(200, 1, 1, 0, 800.0);
    floating.is_floating = true;
    let (mut ctx, shared) = setup_test_wide(
        vec![win1.clone(), floating.clone()],
        2160.0,
        WIDE_IDENTITY,
        1,
    );

    ctx.handle_event(Event::WindowsChanged {
        windows: vec![win1, floating],
    })
    .unwrap();

    let actions = &shared.lock().unwrap().actions;
    assert!(
        consume_left_ids(actions).is_empty(),
        "floating windows must not be stacked: {:?}",
        actions
    );
}

#[test]
fn test_output_change_resizes_to_new_target_once() {
    let (mut ctx, shared) = setup_test_two_outputs(
        three_wide_columns(),
        &[(WIDE_IDENTITY, 3), (LAPTOP_IDENTITY, 2)],
    );

    move_workspace(&mut ctx, &shared, "eDP-1");

    let props = proportion_actions(&shared.lock().unwrap().actions);
    assert_eq!(props.len(), 3, "expected all columns resized: {:?}", props);
    for (_, p) in &props {
        assert!((p - 50.0).abs() < 0.01, "expected halves, got {}", p);
    }

    shared.lock().unwrap().actions.clear();
    ctx.handle_event(Event::WorkspacesChanged {
        workspaces: Vec::new(),
    })
    .unwrap();

    let props = proportion_actions(&shared.lock().unwrap().actions);
    assert!(
        props.is_empty(),
        "repeated event after the change must not resize again: {:?}",
        props
    );
}

#[test]
fn test_output_change_to_single_column_output_stacks() {
    let (mut ctx, shared) = setup_test_two_outputs(
        vec![
            create_mock_window(100, 1, 0, 0, 2560.0),
            create_mock_window(200, 1, 1, 0, 2560.0),
        ],
        &[(WIDE_IDENTITY, 3), (LAPTOP_IDENTITY, 1)],
    );

    move_workspace(&mut ctx, &shared, "eDP-1");

    assert_eq!(
        consume_left_ids(&shared.lock().unwrap().actions),
        vec![200],
        "moving onto a single-column output should stack the workspace"
    );
}

#[test]
fn test_output_change_to_unconfigured_output_uses_default_columns() {
    let (mut ctx, shared) = setup_test_two_outputs(three_wide_columns(), &[(WIDE_IDENTITY, 3)]);

    move_workspace(&mut ctx, &shared, "eDP-1");

    let props = proportion_actions(&shared.lock().unwrap().actions);
    assert_eq!(props.len(), 3, "expected all columns resized: {:?}", props);
    for (_, p) in &props {
        assert!((p - 50.0).abs() < 0.01, "expected halves, got {}", p);
    }
}

#[test]
fn test_output_change_on_inactive_workspace_resizes_without_focusing() {
    let (mut ctx, shared) = setup_test_two_outputs(
        three_wide_columns(),
        &[(WIDE_IDENTITY, 3), (LAPTOP_IDENTITY, 2)],
    );
    shared.lock().unwrap().state.active_workspaces.clear();

    move_workspace(&mut ctx, &shared, "eDP-1");

    let actions = &shared.lock().unwrap().actions;
    assert_eq!(
        proportion_actions(actions).len(),
        3,
        "inactive workspace should still be resized: {:?}",
        actions
    );
    assert!(
        !actions.iter().any(is_focus_action),
        "inactive workspace must not be focused: {:?}",
        actions
    );
}

#[test]
fn test_first_sync_does_not_count_as_output_change() {
    let (mut ctx, shared) = setup_test_two_outputs(
        vec![
            create_mock_window(100, 1, 0, 0, 1500.0),
            create_mock_window(200, 1, 1, 0, 3500.0),
        ],
        &[],
    );

    initial_sync(&mut ctx, &shared);

    let props = proportion_actions(&shared.lock().unwrap().actions);
    assert!(
        props.is_empty(),
        "first sync must not reset columns on an unconfigured output: {:?}",
        props
    );
}

#[test]
fn test_output_width_change_resizes_columns() {
    let win1 = create_mock_window(100, 1, 0, 0, 2560.0);
    let win2 = create_mock_window(200, 1, 1, 0, 2560.0);
    let (mut ctx, shared) = setup_test_two_outputs(vec![win1.clone(), win2], &[(WIDE_IDENTITY, 3)]);
    initial_sync(&mut ctx, &shared);
    {
        let mut shared = shared.lock().unwrap();
        shared.actions.clear();
        shared
            .state
            .output_widths
            .insert("DP-1".to_string(), 2560.0);
    }

    ctx.handle_event(Event::WindowLayoutsChanged {
        changes: vec![(win1.id, win1.layout)],
    })
    .unwrap();

    let props = proportion_actions(&shared.lock().unwrap().actions);
    assert_eq!(
        props.len(),
        2,
        "scale change on the same output should resize columns: {:?}",
        props
    );
}
