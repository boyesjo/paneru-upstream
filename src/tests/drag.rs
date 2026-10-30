use std::time::Duration;

use bevy::ecs::query::Has;
use bevy::prelude::*;
use objc2_core_foundation::CGPoint;

use crate::commands::{Command, Direction, Operation};
use crate::config::{Config, MainOptions};
use crate::ecs::drag::WindowDrag;
use crate::ecs::layout::LayoutStrip;
use crate::ecs::{ActiveWorkspaceMarker, Position, SelectedVirtualMarker, Unmanaged};
use crate::events::{Event, MouseButton};
use crate::manager::{Origin, Window};
use crate::platform::{Modifiers, WinID, WorkspaceId};

use super::*;

const SETTLE: Duration = Duration::from_secs(1);

fn order(h: &mut TestHarness) -> Vec<WinID> {
    let world = h.world();
    let columns = {
        let mut strips = world.query_filtered::<&LayoutStrip, With<ActiveWorkspaceMarker>>();
        strips
            .single(world)
            .expect("one active strip")
            .columns()
            .filter_map(crate::ecs::layout::Column::top)
            .collect::<Vec<_>>()
    };
    let mut windows = world.query::<&Window>();
    columns
        .into_iter()
        .map(|entity| windows.get(world, entity).expect("a window").id())
        .collect()
}

/// The window's frame as the window server has it.
fn os_frame(h: &mut TestHarness, id: WinID) -> IRect {
    let world = h.world();
    let mut windows = world.query::<&Window>();
    windows
        .iter(world)
        .find(|window| window.id() == id)
        .expect("a window")
        .frame()
}

fn send(h: &mut TestHarness, event: Event) {
    h.world().write_message::<Event>(event);
    h.advance(Duration::from_millis(20));
}

fn point(origin: Origin) -> CGPoint {
    CGPoint::new(origin.x.into(), origin.y.into())
}

fn button_down(h: &mut TestHarness, at: Origin, button: MouseButton) {
    send(
        h,
        Event::MouseDown {
            point: point(at),
            modifiers: Modifiers::empty(),
            button,
        },
    );
}

fn button_up(h: &mut TestHarness, at: Origin, button: MouseButton) {
    send(
        h,
        Event::MouseUp {
            point: point(at),
            modifiers: Modifiers::empty(),
            button,
        },
    );
}

fn press(h: &mut TestHarness, at: Origin) {
    button_down(h, at, MouseButton::Left);
}

fn release(h: &mut TestHarness, at: Origin) {
    button_up(h, at, MouseButton::Left);
}

/// Drags `id` by its title bar as the window server does: the pointer moves
/// with the button down, and the window follows it, `dx` at a time.
fn drag(h: &mut TestHarness, id: WinID, from: Origin, steps: &[i32]) -> Origin {
    let steps = steps
        .iter()
        .map(|dx| IVec2::new(*dx, 0))
        .collect::<Vec<_>>();
    drag_by(h, id, from, &steps)
}

/// As [`drag`], a step at a time in any direction.
fn drag_by(h: &mut TestHarness, id: WinID, from: Origin, steps: &[IVec2]) -> Origin {
    let mut at = from;
    for step in steps {
        at += *step;
        h.world().write_message::<Event>(Event::MouseDragged {
            point: point(at),
            modifiers: Modifiers::empty(),
        });
        let frame = os_frame(h, id);
        h.mock_state.os_move_window(id, frame.min + *step);
        h.advance(Duration::from_millis(20));
    }
    at
}

fn x(h: &mut TestHarness, id: WinID) -> i32 {
    os_frame(h, id).min.x
}

fn held(h: &mut TestHarness) -> bool {
    h.world().resource::<WindowDrag>().pressed().is_some()
}

/// The active strip's x.
fn strip_x(h: &mut TestHarness) -> i32 {
    let world = h.world();
    let mut strips = world.query_filtered::<&Position, With<ActiveWorkspaceMarker>>();
    strips.single(world).expect("one active strip").0.x
}

fn dragging(h: &mut TestHarness) -> bool {
    h.world().resource::<WindowDrag>().dragging()
}

/// Three columns, 400 wide, at x 0, 400 and 800 of a 1024-wide display.
fn three() -> TestHarness {
    let mut h = TestHarness::new().with_windows(3);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 1, 2]);
    h
}

/// As [`three`], with windows gliding as Paneru's suggested
/// `animation_speed` has them, not jumping.
fn three_gliding() -> TestHarness {
    let config: Config = (
        MainOptions {
            animation_speed: Some(10.0),
            ..Default::default()
        },
        vec![],
    )
        .into();
    let mut h = TestHarness::new().with_config(config).with_windows(3);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 1, 2]);
    h
}

/// As [`three`], with a second display to the right of the first showing
/// two windows of its own, 100 and 101, at x 1024 and 1424.
fn three_beside_two() -> TestHarness {
    let mut h = TestHarness::new()
        .with_display(
            EXT_DISPLAY_ID,
            IRect::new(
                TEST_DISPLAY_WIDTH,
                0,
                TEST_DISPLAY_WIDTH + EXT_DISPLAY_WIDTH,
                EXT_DISPLAY_HEIGHT,
            ),
            vec![EXT_WORKSPACE_ID],
        )
        .with_windows(3)
        .with_workspace_window(100, EXT_WORKSPACE_ID, |_| {})
        .with_workspace_window(101, EXT_WORKSPACE_ID, |_| {});
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 1, 2]);
    assert_eq!(order_of(&mut h, EXT_WORKSPACE_ID), [100, 101]);
    h
}

/// The columns of the strip the Space `space` shows.
fn order_of(h: &mut TestHarness, space: WorkspaceId) -> Vec<WinID> {
    let world = h.world();
    let columns = {
        let mut strips = world.query::<(&LayoutStrip, Has<SelectedVirtualMarker>)>();
        strips
            .iter(world)
            .filter(|(strip, _)| strip.id() == space)
            .min_by_key(|(strip, selected)| (!*selected, strip.virtual_index))
            .expect("a strip")
            .0
            .columns()
            .filter_map(crate::ecs::layout::Column::top)
            .collect::<Vec<_>>()
    };
    let mut windows = world.query::<&Window>();
    columns
        .into_iter()
        .map(|entity| windows.get(world, entity).expect("a window").id())
        .collect()
}

/// The x of the strip the Space `space` shows.
fn strip_x_of(h: &mut TestHarness, space: WorkspaceId) -> i32 {
    let world = h.world();
    let mut strips = world.query::<(&LayoutStrip, &Position, Has<SelectedVirtualMarker>)>();
    strips
        .iter(world)
        .filter(|(strip, ..)| strip.id() == space)
        .min_by_key(|(strip, _, selected)| (!*selected, strip.virtual_index))
        .expect("a strip")
        .1
        .0
        .x
}

fn landing(h: &mut TestHarness) -> Option<IRect> {
    h.world()
        .resource::<WindowDrag>()
        .landing()
        .map(|(_, frame)| frame)
}

fn active_space(h: &mut TestHarness) -> WorkspaceId {
    let world = h.world();
    let mut strips = world.query_filtered::<&LayoutStrip, With<ActiveWorkspaceMarker>>();
    strips.single(world).expect("one active strip").id()
}

/// Presses window 0's title bar on [`three_beside_two`]'s first display and
/// drags it right until the pointer is on the second, its left edge at 900.
fn onto_the_second(h: &mut TestHarness) -> Origin {
    let start = title_bar(h, 0);
    press(h, start);
    let at = drag(h, 0, start, &[100; 9]);
    assert!(at.x > TEST_DISPLAY_WIDTH);
    h.advance(SETTLE);
    at
}

// Windows 400 wide on the 1024-wide first display are 750 wide on the
// 1920-wide second, and as high as it is below its menu bar, 1180.

fn title_bar(h: &mut TestHarness, id: WinID) -> Origin {
    os_frame(h, id).min + IVec2::new(200, 10)
}

#[test]
fn a_press_on_a_window_is_held_until_release() {
    let mut h = three();
    let at = title_bar(&mut h, 1);
    press(&mut h, at);
    assert!(held(&mut h));
    release(&mut h, at);
    assert!(!held(&mut h));
}

#[test]
fn a_press_on_no_window_holds_nothing() {
    let mut h = three();
    press(&mut h, Origin::new(500, 5));
    assert!(!held(&mut h));
}

#[test]
fn a_press_on_a_floating_window_holds_nothing() {
    let mut h = three();
    let world = h.world();
    let entity = find_window_entity(2, world);
    let mut strips = world.query_filtered::<&mut LayoutStrip, With<ActiveWorkspaceMarker>>();
    strips
        .single_mut(world)
        .expect("one active strip")
        .remove(entity);
    world.entity_mut(entity).insert(Unmanaged::Floating);
    h.advance(SETTLE);
    // Paneru centres it, over window 1: press where only it is.
    let frame = os_frame(&mut h, 2);
    let at = Origin::new(frame.max.x - 20, frame.min.y + 10);
    assert!(at.x > os_frame(&mut h, 1).max.x);
    press(&mut h, at);
    assert!(!held(&mut h));
}

#[test]
fn a_right_click_holds_nothing() {
    let mut h = three();
    let at = title_bar(&mut h, 1);
    button_down(&mut h, at, MouseButton::Right);
    assert!(!held(&mut h));
    button_up(&mut h, at, MouseButton::Right);
    assert!(!held(&mut h));
}

#[test]
fn the_right_button_leaves_a_press_alone() {
    let mut h = three();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    let elsewhere = title_bar(&mut h, 1);
    button_down(&mut h, elsewhere, MouseButton::Right);
    button_up(&mut h, elsewhere, MouseButton::Right);
    // Still window 0's press: dragging it reorders.
    let at = drag(&mut h, 0, start, &[50; 5]);
    h.advance(SETTLE);
    assert!(dragging(&mut h));
    assert_eq!(order(&mut h), [1, 0, 2]);
    release(&mut h, at);
}

#[test]
fn the_right_button_leaves_a_drag_alone() {
    let mut h = three();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    let at = drag(&mut h, 0, start, &[50; 5]);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [1, 0, 2]);
    let elsewhere = title_bar(&mut h, 2);
    button_down(&mut h, elsewhere, MouseButton::Right);
    button_up(&mut h, elsewhere, MouseButton::Right);
    h.advance(SETTLE);
    // Still held where the pointer has it, in the slot it took.
    assert!(dragging(&mut h));
    assert_eq!(order(&mut h), [1, 0, 2]);
    assert_eq!(x(&mut h, 0), 250);
    release(&mut h, at);
    h.advance(SETTLE);
    assert!(!held(&mut h));
    assert_eq!(x(&mut h, 0), 400);
}

#[test]
fn a_click_reorders_nothing() {
    let mut h = three();
    let at = title_bar(&mut h, 0);
    press(&mut h, at);
    release(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 1, 2]);
    assert_eq!(x(&mut h, 0), 0);
}

#[test]
fn an_app_moving_its_own_window_is_no_drag() {
    let mut h = three();
    let at = title_bar(&mut h, 0);
    press(&mut h, at);
    for _ in 0..10 {
        let frame = os_frame(&mut h, 0);
        h.mock_state
            .os_move_window(0, frame.min + IVec2::new(50, 0));
        h.advance(Duration::from_millis(20));
    }
    release(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 1, 2]);
}

#[test]
fn an_edge_resize_is_no_drag() {
    let mut h = three();
    let mut at = os_frame(&mut h, 1).min + IVec2::new(1, 300);
    press(&mut h, at);
    for _ in 0..6 {
        at.x += 50;
        h.world().write_message::<Event>(Event::MouseDragged {
            point: point(at),
            modifiers: Modifiers::empty(),
        });
        let frame = os_frame(&mut h, 1);
        h.mock_state
            .os_move_window(1, frame.min + IVec2::new(50, 0));
        h.mock_state
            .os_resize_window(1, frame.size() - IVec2::new(50, 0));
        h.advance(Duration::from_millis(20));
    }
    release(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 1, 2]);
}

#[test]
fn past_a_neighbours_middle_the_column_takes_its_slot() {
    let mut h = three();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    // Its left edge at 150 is nearer its own slot at 0 than the next at 400.
    let at = drag(&mut h, 0, start, &[50, 50, 50]);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 1, 2]);
    // At 250 it is nearer 400: window 1 glides into the slot it left.
    drag(&mut h, 0, at, &[50, 50]);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [1, 0, 2]);
    assert_eq!(x(&mut h, 1), 0);
    // Paneru leaves the held window where the pointer has it.
    assert_eq!(x(&mut h, 0), 250);
}

#[test]
fn dragged_back_the_column_goes_back() {
    let mut h = three();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    let at = drag(&mut h, 0, start, &[50; 10]);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [1, 0, 2]);
    drag(&mut h, 0, at, &[-50; 10]);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 1, 2]);
    assert_eq!(x(&mut h, 1), 400);
}

#[test]
fn a_release_lands_the_window_in_its_slot() {
    let mut h = three();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    let at = drag(&mut h, 0, start, &[50; 10]);
    release(&mut h, at);
    h.advance(SETTLE);
    assert!(!held(&mut h));
    assert_eq!(order(&mut h), [1, 0, 2]);
    assert_eq!(x(&mut h, 1), 0);
    assert_eq!(x(&mut h, 0), 400);
}

#[test]
fn a_lost_release_ends_the_drag_as_a_release() {
    let mut h = three();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    let at = drag(&mut h, 0, start, &[50; 10]);
    send(
        &mut h,
        Event::MouseMoved {
            point: point(at),
            modifiers: Modifiers::empty(),
        },
    );
    h.advance(SETTLE);
    assert!(!held(&mut h));
    assert_eq!(order(&mut h), [1, 0, 2]);
    assert_eq!(x(&mut h, 0), 400);
}

#[test]
fn off_its_display_the_column_goes_back_to_where_it_began() {
    let mut h = three();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    let at = drag(&mut h, 0, start, &[50; 10]);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [1, 0, 2]);
    // The pointer past the display's right edge, at 1024.
    let at = drag(&mut h, 0, at, &[50; 7]);
    assert!(at.x > 1024);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 1, 2]);
    release(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 1, 2]);
    assert_eq!(x(&mut h, 0), 0);
}

#[test]
fn a_pointer_in_the_menu_bar_is_on_its_display() {
    let mut h = three();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    let in_menu_bar = Origin::new(start.x, TEST_MENUBAR_HEIGHT / 2);
    let at = drag(&mut h, 0, in_menu_bar, &[50; 10]);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [1, 0, 2]);
    release(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(x(&mut h, 0), 400);
}

#[test]
fn the_pointer_not_the_window_decides_the_display() {
    let mut h = three();
    // Held near its left edge, the window's middle passes the display's
    // right edge, at 1024, well before the pointer does.
    let start = os_frame(&mut h, 0).min + IVec2::new(20, 10);
    press(&mut h, start);
    let at = drag(&mut h, 0, start, &[50; 18]);
    assert!(os_frame(&mut h, 0).center().x > 1024);
    assert!(at.x < 1024);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [1, 2, 0]);
    release(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [1, 2, 0]);
}

#[test]
fn a_window_closed_while_dragged_ends_the_drag() {
    let mut h = three();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    drag(&mut h, 0, start, &[50; 10]);
    h.mock_state.os_close_window(0);
    h.advance(SETTLE);
    assert!(!held(&mut h));
    assert_eq!(order(&mut h), [1, 2]);
}

#[test]
fn a_lone_column_glides_back() {
    let mut h = TestHarness::new().with_windows(1);
    h.advance(SETTLE);
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    let at = drag(&mut h, 0, start, &[50; 6]);
    assert_eq!(x(&mut h, 0), 300);
    release(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(x(&mut h, 0), 0);
}

#[test]
fn the_landing_is_the_slot_the_window_will_glide_into() {
    let mut h = three();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    let at = drag(&mut h, 0, start, &[50; 10]);
    h.advance(SETTLE);
    let landing = h.world().resource::<WindowDrag>().landing();
    let (window_id, frame) = landing.expect("a landing while dragged");
    assert_eq!(window_id, 0);
    assert_eq!(frame.min.x, 400);
    assert_eq!(frame.size(), os_frame(&mut h, 0).size());
    release(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(h.world().resource::<WindowDrag>().landing(), None);
    assert_eq!(os_frame(&mut h, 0).min, frame.min);
}

#[test]
fn a_stacked_window_carries_its_column() {
    let mut h = three();
    for command in [
        Command::Window(Operation::Focus(Direction::First)),
        Command::Window(Operation::Focus(Direction::East)),
        Command::Window(Operation::Stack(true)),
    ] {
        send(&mut h, Event::Command { command });
    }
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 2]);
    let lower = os_frame(&mut h, 1);
    assert!(lower.min.y > os_frame(&mut h, 0).min.y, "1 is below 0");
    let start = title_bar(&mut h, 1);
    press(&mut h, start);
    let at = drag(&mut h, 1, start, &[50; 10]);
    h.advance(SETTLE);
    // Window 0, its row above, goes with the column; window 1 stays put.
    assert_eq!(order(&mut h), [2, 0]);
    assert_eq!(x(&mut h, 0), 400);
    assert_eq!(x(&mut h, 1), 500);
    release(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(os_frame(&mut h, 1).min, IVec2::new(400, lower.min.y));
}

#[test]
fn the_slots_move_with_the_strip_under_a_held_window() {
    let mut h = three();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    // Its left edge at 100 is nearest its own slot at 0.
    let at = drag(&mut h, 0, start, &[50, 50]);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 1, 2]);
    // Bringing window 2 into view slides the strip left under the held
    // window, so the second slot is now the nearest.
    send(
        &mut h,
        Event::Command {
            command: Command::Window(Operation::Focus(Direction::Last)),
        },
    );
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [1, 0, 2]);
    assert_eq!(x(&mut h, 0), 100);
    release(&mut h, at);
    h.advance(SETTLE);
    assert!(!held(&mut h));
    let next_to_1 = x(&mut h, 1) + 400;
    assert_eq!(x(&mut h, 0), next_to_1);
}

#[test]
fn a_relayout_leaves_the_held_window_alone() {
    let mut h = three();
    let start = title_bar(&mut h, 2);
    press(&mut h, start);
    let at = drag(&mut h, 2, start, &[-50, -50]);
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 1, 2]);
    // Window 0 narrowed by 200 lays the strip out again: window 1 follows
    // it, and the held window stays where the pointer has it.
    let size = os_frame(&mut h, 0).size();
    h.mock_state.os_resize_window(0, size - IVec2::new(200, 0));
    h.advance(SETTLE);
    let after_0 = x(&mut h, 0) + 200;
    assert_eq!(x(&mut h, 1), after_0);
    assert_eq!(x(&mut h, 2), 700);
    release(&mut h, at);
    h.advance(SETTLE);
    let after_1 = x(&mut h, 1) + 400;
    assert_eq!(x(&mut h, 2), after_1);
}

// The strip of `three` is 1200 wide on a 1024-wide display: it scrolls as far
// as x -176, its right end at the display's right edge.

#[test]
fn at_the_edge_the_strip_scrolls_to_the_columns_beyond() {
    let mut h = three_gliding();
    let start = title_bar(&mut h, 2);
    press(&mut h, start);
    let at = drag(&mut h, 2, start, &[10, 10]);
    assert!(at.x >= 1024 - 48);
    h.advance(SETTLE);
    assert_eq!(strip_x(&mut h), -176);
    assert_eq!(order(&mut h), [0, 1, 2]);
    assert_eq!(x(&mut h, 2), 820);
    release(&mut h, at);
    h.advance(SETTLE);
    assert!(!held(&mut h));
    assert_eq!(x(&mut h, 2), 800 - 176);
}

#[test]
fn the_columns_not_passed_move_with_the_strip() {
    let mut h = three_gliding();
    let start = title_bar(&mut h, 2);
    press(&mut h, start);
    let at = drag(&mut h, 2, start, &[10, 10]);
    for _ in 0..10 {
        h.advance(Duration::from_millis(20));
        let strip = strip_x(&mut h);
        assert_eq!(x(&mut h, 0), strip);
        assert_eq!(x(&mut h, 1), strip + 400);
    }
    // Under way, not at its end.
    let strip = strip_x(&mut h);
    assert!(-176 < strip && strip < 0, "{strip}");
    release(&mut h, at);
}

#[test]
fn a_press_at_the_edge_scrolls_nothing() {
    let mut h = three_gliding();
    let at = Origin::new(1020, 30);
    press(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(strip_x(&mut h), 0);
    release(&mut h, at);
}

#[test]
fn at_the_strips_end_the_edge_scrolls_nothing() {
    let mut h = three_gliding();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    let at = drag(&mut h, 0, start, &[-50; 4]);
    assert!(at.x < 48);
    h.advance(SETTLE);
    assert_eq!(strip_x(&mut h), 0);
    assert_eq!(order(&mut h), [0, 1, 2]);
    release(&mut h, at);
}

#[test]
fn leaving_the_edge_stops_the_scroll() {
    let mut h = three_gliding();
    let start = title_bar(&mut h, 2);
    press(&mut h, start);
    let at = drag(&mut h, 2, start, &[10, 10]);
    h.advance(Duration::from_millis(200));
    let at = drag(&mut h, 2, at, &[-100]);
    let left_at = strip_x(&mut h);
    assert!(-176 < left_at && left_at < 0, "{left_at}");
    h.advance(SETTLE);
    assert_eq!(strip_x(&mut h), left_at);
    release(&mut h, at);
}

#[test]
fn a_release_at_the_edge_lands_the_window_in_its_slot() {
    let mut h = three_gliding();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    // Past both neighbours, its left edge at 800.
    let at = drag(&mut h, 0, start, &[50; 16]);
    assert!(at.x >= 1024 - 48);
    h.advance(Duration::from_millis(200));
    assert_eq!(order(&mut h), [1, 2, 0]);
    release(&mut h, at);
    h.advance(SETTLE);
    assert!(!held(&mut h));
    assert_eq!(order(&mut h), [1, 2, 0]);
    let strip = strip_x(&mut h);
    assert_eq!(x(&mut h, 0), strip + 800);
    assert_eq!(x(&mut h, 1), strip);
}

#[test]
fn over_another_display_the_column_joins_its_strip() {
    let mut h = three_beside_two();
    onto_the_second(&mut h);
    // Its own strip closes up, and the other's parts for it at the slot
    // nearest its left edge, as wide as its share of that display.
    assert_eq!(order_of(&mut h, TEST_WORKSPACE_ID), [1, 2]);
    assert_eq!(order_of(&mut h, EXT_WORKSPACE_ID), [0, 100, 101]);
    assert_eq!(x(&mut h, 1), 0);
    assert_eq!(x(&mut h, 100), TEST_DISPLAY_WIDTH + 750);
    assert_eq!(x(&mut h, 101), TEST_DISPLAY_WIDTH + 750 + 400);
    let gap = landing(&mut h).expect("a landing there");
    assert_eq!(gap.min, IVec2::new(TEST_DISPLAY_WIDTH, TEST_MENUBAR_HEIGHT));
    assert_eq!(gap.size(), IVec2::new(750, 1180));
    // The held window keeps its size, and stays where the pointer has it.
    let held = os_frame(&mut h, 0);
    assert_eq!(held.size(), IVec2::new(400, 748));
    assert_eq!(held.min.x, 900);
}

#[test]
fn a_carried_column_follows_the_slots_there() {
    let mut h = three_beside_two();
    let at = onto_the_second(&mut h);
    // Its left edge at 1400, nearest the slot past window 100, at 1424.
    drag(&mut h, 0, at, &[100; 5]);
    h.advance(SETTLE);
    assert_eq!(order_of(&mut h, EXT_WORKSPACE_ID), [100, 0, 101]);
    assert_eq!(x(&mut h, 100), TEST_DISPLAY_WIDTH);
    assert_eq!(x(&mut h, 101), TEST_DISPLAY_WIDTH + 400 + 750);
}

#[test]
fn back_on_its_display_the_column_rejoins_its_strip() {
    let mut h = three_beside_two();
    let at = onto_the_second(&mut h);
    // Its left edge at 600: a tie between the slots at 400 and 800 goes to
    // the earlier.
    let at = drag(&mut h, 0, at, &[-100; 3]);
    assert!(at.x < TEST_DISPLAY_WIDTH);
    h.advance(SETTLE);
    assert_eq!(order_of(&mut h, TEST_WORKSPACE_ID), [1, 0, 2]);
    assert_eq!(order_of(&mut h, EXT_WORKSPACE_ID), [100, 101]);
    assert_eq!(x(&mut h, 100), TEST_DISPLAY_WIDTH);
    assert_eq!(landing(&mut h).expect("a landing").width(), 400);
    release(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(os_frame(&mut h, 0).min.x, 400);
    assert_eq!(os_frame(&mut h, 0).size(), IVec2::new(400, 748));
}

#[test]
fn let_go_on_another_display_the_window_lands_in_the_gap_there() {
    let mut h = three_beside_two();
    let at = onto_the_second(&mut h);
    release(&mut h, at);
    h.advance(SETTLE);
    assert!(!held(&mut h));
    assert_eq!(order_of(&mut h, EXT_WORKSPACE_ID), [0, 100, 101]);
    assert_eq!(
        os_frame(&mut h, 0),
        IRect::new(
            TEST_DISPLAY_WIDTH,
            TEST_MENUBAR_HEIGHT,
            TEST_DISPLAY_WIDTH + 750,
            EXT_DISPLAY_HEIGHT
        )
    );
    assert_eq!(x(&mut h, 100), TEST_DISPLAY_WIDTH + 750);
}

#[test]
fn let_go_on_another_display_a_focused_window_takes_focus_there() {
    let mut h = three_beside_two();
    h.mock_state.focus_window(0);
    h.advance(SETTLE);
    assert_eq!(active_space(&mut h), TEST_WORKSPACE_ID);
    let at = onto_the_second(&mut h);
    release(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(active_space(&mut h), EXT_WORKSPACE_ID);
    assert_eq!(order(&mut h), [0, 100, 101]);
}

#[test]
fn its_old_strip_shown_again_keeps_none_of_the_column() {
    let mut h = three_beside_two();
    h.mock_state.focus_window(100);
    h.advance(SETTLE);
    assert_eq!(active_space(&mut h), EXT_WORKSPACE_ID);
    let at = onto_the_second(&mut h);
    // Activated, the strip it left looks for windows on its Space it hasn't
    // got, and macOS lists window 0 there until it's let go and moved.
    h.mock_state.focus_window(1);
    h.advance(SETTLE);
    assert_eq!(active_space(&mut h), TEST_WORKSPACE_ID);
    assert_eq!(order_of(&mut h, EXT_WORKSPACE_ID), [0, 100, 101]);
    release(&mut h, at);
    h.advance(Duration::from_millis(200));
    h.mock_state.focus_window(100);
    h.advance(Duration::from_millis(200));
    h.mock_state.focus_window(1);
    h.advance(Duration::from_millis(200));
    assert_eq!(active_space(&mut h), TEST_WORKSPACE_ID);
    assert_eq!(order_of(&mut h, TEST_WORKSPACE_ID), [1, 2]);
    assert_eq!(order_of(&mut h, EXT_WORKSPACE_ID), [0, 100, 101]);
}

#[test]
fn its_old_strip_shown_mid_drag_keeps_none_of_a_stacked_column() {
    let mut h = three_beside_two();
    for command in [
        Command::Window(Operation::Focus(Direction::First)),
        Command::Window(Operation::Focus(Direction::East)),
        Command::Window(Operation::Stack(true)),
    ] {
        send(&mut h, Event::Command { command });
    }
    h.mock_state.focus_window(1);
    h.advance(SETTLE);
    h.mock_state.focus_window(100);
    h.advance(SETTLE);
    assert_eq!(active_space(&mut h), EXT_WORKSPACE_ID);
    let start = title_bar(&mut h, 1);
    press(&mut h, start);
    drag(&mut h, 1, start, &[100; 9]);
    // Activated, the strip it left finds the two windows macOS still lists
    // on its Space and would take them back as a workspace of their own,
    // left there empty once the drag carries them on.
    h.mock_state.focus_window(2);
    h.advance(SETTLE);
    assert_eq!(active_space(&mut h), TEST_WORKSPACE_ID);
    assert_eq!(order_of(&mut h, TEST_WORKSPACE_ID), [2]);
    assert_eq!(order_of(&mut h, EXT_WORKSPACE_ID), [0, 100, 101]);
    let world = h.world();
    let mut strips = world.query::<&LayoutStrip>();
    let rows = strips
        .iter(world)
        .filter(|strip| strip.id() == TEST_WORKSPACE_ID)
        .count();
    assert_eq!(rows, 1);
}

const FULLSCREEN_WORKSPACE_ID: WorkspaceId = 30;

/// [`three_beside_two`] with the second display showing window 101 full
/// screen, on a Space of its own.
fn three_beside_a_full_screen_one() -> TestHarness {
    full_screen_on_the_second(three_beside_two())
}

/// The second display showing window 101 full screen, on a Space of its own.
fn full_screen_on_the_second(mut h: TestHarness) -> TestHarness {
    h.mock_state.activate_display(EXT_DISPLAY_ID);
    h.mock_state.focus_window(101);
    h.advance(SETTLE);
    h.mock_state.update_window(101, |window| {
        window.workspace_id = FULLSCREEN_WORKSPACE_ID;
        window.is_full_screen = true;
    });
    h.mock_state
        .activate_workspace(EXT_DISPLAY_ID, FULLSCREEN_WORKSPACE_ID, true);
    h.world().write_message::<Event>(Event::SpaceChanged);
    h.advance(SETTLE);
    h
}

#[test]
fn over_a_full_screen_space_the_column_goes_back() {
    let mut h = three_beside_a_full_screen_one();
    onto_the_second(&mut h);
    assert_eq!(order_of(&mut h, TEST_WORKSPACE_ID), [0, 1, 2]);
    assert_eq!(order_of(&mut h, FULLSCREEN_WORKSPACE_ID), [101]);
    // Where it will go if let go there.
    assert_eq!(landing(&mut h).map(|frame| frame.min.x), Some(0));
}

#[test]
fn let_go_over_a_full_screen_space_the_window_glides_back() {
    let mut h = three_beside_a_full_screen_one();
    let at = onto_the_second(&mut h);
    // macOS keeps it off that display, at the edge of its own.
    let y = os_frame(&mut h, 0).min.y;
    h.mock_state
        .os_move_window(0, Origin::new(TEST_DISPLAY_WIDTH - 400, y));
    h.advance(Duration::from_millis(20));
    release(&mut h, at);
    h.advance(SETTLE);
    assert!(!held(&mut h));
    assert_eq!(order_of(&mut h, TEST_WORKSPACE_ID), [0, 1, 2]);
    assert_eq!(x(&mut h, 0), 0);
}

#[test]
fn let_go_on_a_full_screen_space_a_window_macos_put_there_stays() {
    let mut h = three_beside_a_full_screen_one();
    let at = onto_the_second(&mut h);
    release(&mut h, at);
    h.advance(SETTLE);
    assert!(!held(&mut h));
    assert_eq!(x(&mut h, 0), 900);
}

#[test]
fn onto_a_third_display_the_column_goes_on() {
    const THIRD_DISPLAY_ID: u32 = 3;
    const THIRD_WORKSPACE_ID: WorkspaceId = 21;
    let third = TEST_DISPLAY_WIDTH + EXT_DISPLAY_WIDTH;
    let mut h = TestHarness::new()
        .with_display(
            EXT_DISPLAY_ID,
            IRect::new(TEST_DISPLAY_WIDTH, 0, third, EXT_DISPLAY_HEIGHT),
            vec![EXT_WORKSPACE_ID],
        )
        .with_display(
            THIRD_DISPLAY_ID,
            IRect::new(third, 0, third + TEST_DISPLAY_WIDTH, TEST_DISPLAY_HEIGHT),
            vec![THIRD_WORKSPACE_ID],
        )
        .with_windows(3)
        .with_workspace_window(100, EXT_WORKSPACE_ID, |_| {})
        .with_workspace_window(200, THIRD_WORKSPACE_ID, |_| {});
    h.advance(SETTLE);
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    let at = drag(&mut h, 0, start, &[100; 20]);
    assert!(TEST_DISPLAY_WIDTH < at.x && at.x < third);
    h.advance(SETTLE);
    assert_eq!(order_of(&mut h, EXT_WORKSPACE_ID), [100, 0]);
    // Past the second display's right edge, its left edge at 2800.
    let at = drag(&mut h, 0, at, &[100; 8]);
    assert!(at.x > third);
    h.advance(SETTLE);
    assert_eq!(order_of(&mut h, EXT_WORKSPACE_ID), [100]);
    assert_eq!(order_of(&mut h, THIRD_WORKSPACE_ID), [0, 200]);
    assert_eq!(landing(&mut h).expect("a landing").width(), 400);
    assert_eq!(x(&mut h, 200), third + 400);
    release(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(x(&mut h, 0), third);
}

#[test]
fn a_pointer_in_another_displays_menu_bar_is_on_it() {
    let mut h = TestHarness::new()
        .with_display(
            EXT_DISPLAY_ID,
            IRect::new(0, -EXT_DISPLAY_HEIGHT, EXT_DISPLAY_WIDTH, 0),
            vec![EXT_WORKSPACE_ID],
        )
        .with_windows(3)
        .with_workspace_window(100, EXT_WORKSPACE_ID, |_| {});
    h.advance(SETTLE);
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    let to_menu_bar = -EXT_DISPLAY_HEIGHT + TEST_MENUBAR_HEIGHT / 2 - start.y;
    drag_by(&mut h, 0, start, &[IVec2::new(0, to_menu_bar)]);
    h.advance(SETTLE);
    assert_eq!(order_of(&mut h, EXT_WORKSPACE_ID), [0, 100]);
}

#[test]
fn a_window_closed_while_carried_ends_the_drag() {
    let mut h = three_beside_two();
    onto_the_second(&mut h);
    h.mock_state.os_close_window(0);
    h.advance(SETTLE);
    assert!(!held(&mut h));
    assert_eq!(order_of(&mut h, EXT_WORKSPACE_ID), [100, 101]);
    assert_eq!(order_of(&mut h, TEST_WORKSPACE_ID), [1, 2]);
}

#[test]
fn a_stacked_column_carries_its_rows_there() {
    let mut h = three_beside_two();
    for command in [
        Command::Window(Operation::Focus(Direction::First)),
        Command::Window(Operation::Focus(Direction::East)),
        Command::Window(Operation::Stack(true)),
    ] {
        send(&mut h, Event::Command { command });
    }
    h.advance(SETTLE);
    assert_eq!(order(&mut h), [0, 2]);
    let lower = os_frame(&mut h, 1);
    let start = title_bar(&mut h, 1);
    press(&mut h, start);
    let at = drag(&mut h, 1, start, &[100; 9]);
    assert!(at.x > TEST_DISPLAY_WIDTH);
    h.advance(SETTLE);
    assert_eq!(order_of(&mut h, EXT_WORKSPACE_ID), [0, 100, 101]);
    // Window 0, its row above, glides into the gap at that display's size;
    // window 1 stays under the pointer.
    let upper = os_frame(&mut h, 0);
    assert_eq!(
        upper.min,
        IVec2::new(TEST_DISPLAY_WIDTH, TEST_MENUBAR_HEIGHT)
    );
    assert_eq!(upper.width(), 750);
    assert_eq!(os_frame(&mut h, 1).size(), lower.size());
    release(&mut h, at);
    h.advance(SETTLE);
    let lower = os_frame(&mut h, 1);
    assert_eq!(lower.min.x, TEST_DISPLAY_WIDTH);
    assert_eq!(lower.width(), 750);
    assert_eq!(lower.max.y, EXT_DISPLAY_HEIGHT);
}

/// A window on the first display, beside a second showing five, 100 to 104,
/// which reach past its right edge; windows glide.
fn one_beside_five() -> TestHarness {
    let config: Config = (
        MainOptions {
            animation_speed: Some(10.0),
            ..Default::default()
        },
        vec![],
    )
        .into();
    let mut h = TestHarness::new()
        .with_config(config)
        .with_display(
            EXT_DISPLAY_ID,
            IRect::new(
                TEST_DISPLAY_WIDTH,
                0,
                TEST_DISPLAY_WIDTH + EXT_DISPLAY_WIDTH,
                EXT_DISPLAY_HEIGHT,
            ),
            vec![EXT_WORKSPACE_ID],
        )
        .with_windows(1);
    for id in 100..105 {
        h = h.with_workspace_window(id, EXT_WORKSPACE_ID, |_| {});
    }
    h.advance(SETTLE);
    h
}

#[test]
fn at_another_displays_edge_its_strip_scrolls() {
    let mut h = one_beside_five();
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    // To the second display's right edge, at 2944.
    let at = drag(&mut h, 0, start, &[300; 9]);
    assert!(at.x >= TEST_DISPLAY_WIDTH + EXT_DISPLAY_WIDTH - 48);
    h.advance(SETTLE);
    let world = h.world();
    let mut strips = world.query::<(&LayoutStrip, &Position)>();
    let (_, position) = strips
        .iter(world)
        .find(|(strip, _)| strip.id() == EXT_WORKSPACE_ID)
        .expect("its strip");
    assert!(position.0.x < TEST_DISPLAY_WIDTH, "{}", position.0.x);
    release(&mut h, at);
}

#[test]
fn a_strip_the_column_leaves_goes_back_where_it_was() {
    let mut h = one_beside_five();
    let before = strip_x_of(&mut h, EXT_WORKSPACE_ID);
    let start = title_bar(&mut h, 0);
    press(&mut h, start);
    // To the second display's right edge, which scrolls its strip.
    let at = drag(&mut h, 0, start, &[300; 9]);
    h.advance(SETTLE);
    assert!(strip_x_of(&mut h, EXT_WORKSPACE_ID) < before);
    // Back on the first display.
    let at = drag(&mut h, 0, at, &[-300; 8]);
    assert!(at.x < TEST_DISPLAY_WIDTH);
    h.advance(SETTLE);
    assert_eq!(
        order_of(&mut h, EXT_WORKSPACE_ID),
        [100, 101, 102, 103, 104]
    );
    assert_eq!(strip_x_of(&mut h, EXT_WORKSPACE_ID), before);
    release(&mut h, at);
    h.advance(SETTLE);
    assert_eq!(strip_x_of(&mut h, EXT_WORKSPACE_ID), before);
}

/// As [`three_gliding`], with a second display to the left of the first
/// showing windows 100 and 101; the first's right side has none beyond.
fn three_right_of_two() -> TestHarness {
    let config: Config = (
        MainOptions {
            animation_speed: Some(10.0),
            ..Default::default()
        },
        vec![],
    )
        .into();
    let mut h = TestHarness::new()
        .with_config(config)
        .with_display(
            EXT_DISPLAY_ID,
            IRect::new(-EXT_DISPLAY_WIDTH, 0, 0, EXT_DISPLAY_HEIGHT),
            vec![EXT_WORKSPACE_ID],
        )
        .with_windows(3)
        .with_workspace_window(100, EXT_WORKSPACE_ID, |_| {})
        .with_workspace_window(101, EXT_WORKSPACE_ID, |_| {});
    h.advance(SETTLE);
    assert_eq!(order_of(&mut h, TEST_WORKSPACE_ID), [0, 1, 2]);
    h
}

/// Drags window 2 of [`three_right_of_two`] to the first display's right
/// edge, which scrolls its strip to its end, then onto the second display.
fn scrolled_then_onto_the_left(h: &mut TestHarness) -> Origin {
    let start = title_bar(h, 2);
    press(h, start);
    let at = drag(h, 2, start, &[10, 10]);
    h.advance(SETTLE);
    assert_eq!(strip_x_of(h, TEST_WORKSPACE_ID), -176);
    let at = drag(h, 2, at, &[-300; 5]);
    assert!(at.x < 0);
    h.advance(SETTLE);
    at
}

#[test]
fn dropped_on_another_display_its_own_strip_goes_back_where_it_was() {
    let mut h = three_right_of_two();
    let at = scrolled_then_onto_the_left(&mut h);
    assert_eq!(order_of(&mut h, TEST_WORKSPACE_ID), [0, 1]);
    assert_eq!(strip_x_of(&mut h, TEST_WORKSPACE_ID), 0);
    release(&mut h, at);
    h.advance(SETTLE);
    assert!(order_of(&mut h, EXT_WORKSPACE_ID).contains(&2));
    assert_eq!(strip_x_of(&mut h, TEST_WORKSPACE_ID), 0);
}

#[test]
fn over_a_full_screen_space_its_own_strip_goes_back_where_it_was() {
    let mut h = full_screen_on_the_second(three_right_of_two());
    let at = scrolled_then_onto_the_left(&mut h);
    assert_eq!(order_of(&mut h, TEST_WORKSPACE_ID), [0, 1, 2]);
    assert_eq!(strip_x_of(&mut h, TEST_WORKSPACE_ID), 0);
    release(&mut h, at);
}
