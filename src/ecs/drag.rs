//! Dragging a tiled window by its title bar moves its column along the strip.
//!
//! The window server moves the window with the pointer. Paneru leaves it there
//! and keeps its column in the slot nearest it, so the other columns glide
//! aside and the gap they leave is where it will land. On release it glides
//! into the gap. Held at a display's edge, the window scrolls the strip under
//! it, to columns beyond the edge.

use bevy::app::{App, Update};
use bevy::ecs::change_detection::DetectChangesMut as _;
use bevy::ecs::entity::Entity;
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::message::MessageReader;
use bevy::ecs::query::{Has, With, Without};
use bevy::ecs::resource::Resource;
use bevy::ecs::schedule::IntoScheduleConfigs as _;
use bevy::ecs::schedule::common_conditions::on_message;
use bevy::ecs::system::{Commands, Query, Res, ResMut};
use bevy::math::IRect;
use bevy::time::Time;
use std::time::Instant;
use tracing::debug;

use crate::config::Config;
use crate::ecs::layout::LayoutStrip;
use crate::ecs::workspace::spawn_space_move_guard;
use crate::ecs::{
    ActiveWorkspaceMarker, Bounds, DockPosition, FocusedMarker, LayoutPosition,
    NativeFullscreenMarker, Position, RepositionMarker, ResizeMarker, Scrolling,
    SelectedVirtualMarker, SpawnCommandsExt as _, UpdateSet, VerifyWindowPosition, WidthRatio,
};
use crate::events::{Event, InputEvent, MouseButton};
use crate::manager::{Display, Origin, Window, WindowManager, origin_from};
use crate::platform::{WinID, WorkspaceId};
use crate::util::round_px;

/// How near a display's edge the pointer scrolls the strip, in points, at an
/// edge with no display beyond it.
const EDGE_ZONE: f64 = 48.0;

/// How fast the strip scrolls with the pointer at the very edge, in display
/// widths a second, and slower further in: a column a third of the display
/// wide passes in a third of a second.
const EDGE_SPEED: f64 = 1.0;

/// How long the scroll takes to come up to speed, in seconds, so a window
/// flung to the edge doesn't skip a column.
const EDGE_RAMP: f64 = 0.25;

/// A press of the mouse button on a window, from the press until the button
/// comes up, and the drag it may turn into.
#[derive(Default, Resource)]
pub struct WindowDrag {
    press: Option<Press>,
    /// The dragged window and the frame it will land in, for subscribers.
    landing: Option<(WinID, IRect)>,
}

struct Press {
    window: Entity,
    /// When the button went down, for the log.
    at: Instant,
    /// The window's frame as the press began, or as Paneru last put it.
    frame: IRect,
    /// Where the pointer last moved with the button down, if it has.
    pointer: Option<Origin>,
    /// The button came up, or the pointer moved without it.
    released: bool,
    phase: Phase,
    /// The strip scrolling while the pointer is at a display's edge.
    edge: Option<EdgeScroll>,
}

#[derive(Default)]
struct EdgeScroll {
    /// How long the pointer has been at the edge, in seconds.
    held_for: f64,
    /// The fraction of a pixel the strip is still to move.
    carry: f64,
    /// How far the strip has scrolled, for the log.
    scrolled: i32,
}

enum Phase {
    Pressed,
    Dragging {
        /// The column's index as the drag began, where it goes back to while
        /// the pointer is over no strip.
        origin: usize,
        /// The window and any native tabs sharing its frame: the layout
        /// leaves them where the pointer has them.
        held: Vec<Entity>,
        /// The strip the column began in.
        home: Entity,
        /// The column's windows as the drag began, which a strip on another
        /// display holds before macOS gives them that display's Space.
        column: Vec<Entity>,
        /// The display the pointer was last on, and the Space it shows.
        over: Option<(Entity, Option<WorkspaceId>)>,
        /// The pointer is on a display with no strip to take the column, as
        /// one showing a full-screen Space. macOS keeps windows off that, so
        /// let go there, the window glides back to its slot; one macOS did
        /// put there is left to that display's Space.
        away: bool,
        /// Where each strip the column has been in was heading as it came,
        /// its own as the drag began: a strip it leaves goes back there.
        scrolled_from: Vec<(Entity, Origin)>,
    },
}

impl WindowDrag {
    /// A press of the button on `window`, whose frame is `frame`, or
    /// on no managed window. A second button pressed during a drag changes
    /// nothing.
    pub(crate) fn press(&mut self, window: Option<(Entity, IRect)>) {
        if self.dragging() {
            return;
        }
        self.press = window.map(|(window, frame)| Press {
            window,
            at: Instant::now(),
            frame,
            pointer: None,
            released: false,
            phase: Phase::Pressed,
            edge: None,
        });
    }

    /// Whether `entity` is held by a drag, so the layout leaves it alone.
    pub(crate) fn holds(&self, entity: Entity) -> bool {
        self.press.as_ref().is_some_and(|press| match &press.phase {
            Phase::Dragging { held, .. } => held.contains(&entity),
            Phase::Pressed => false,
        })
    }

    /// Whether `entity` is in the column being dragged, which may be in a
    /// strip of another display than the Space macOS still lists it on.
    pub(crate) fn carries(&self, entity: Entity) -> bool {
        self.press.as_ref().is_some_and(|press| match &press.phase {
            Phase::Dragging { column, .. } => column.contains(&entity),
            Phase::Pressed => false,
        })
    }

    /// The window being dragged and the frame it will land in.
    pub(crate) fn landing(&self) -> Option<(WinID, IRect)> {
        self.landing
    }

    /// Whether a window is being dragged, not only pressed.
    pub(crate) fn dragging(&self) -> bool {
        self.press
            .as_ref()
            .is_some_and(|press| matches!(press.phase, Phase::Dragging { .. }))
    }

    #[cfg(test)]
    pub(crate) fn pressed(&self) -> Option<Entity> {
        self.press.as_ref().map(|press| press.window)
    }
}

pub(super) fn register(app: &mut App) {
    let pressed = |drag: Res<WindowDrag>| drag.press.is_some();
    let reporting = |drag: Res<WindowDrag>| drag.press.is_some() || drag.landing.is_some();
    app.init_resource::<WindowDrag>();
    app.add_systems(
        Update,
        (
            track_button
                .after(super::mouse::mouse_down_trigger)
                .in_set(UpdateSet::Input)
                .run_if(on_message::<InputEvent>),
            (edge_scroll, drag_window)
                .chain()
                .in_set(UpdateSet::Workspace)
                .run_if(pressed),
            report_landing
                .in_set(UpdateSet::Housekeeping)
                .run_if(reporting),
        ),
    );
}

/// Follows the left button after a press: the pointer moving with it down, and
/// its release. The pointer moving with no button down means the release was
/// lost. The right button's release is no end of a drag.
fn track_button(mut messages: MessageReader<InputEvent>, mut drag: ResMut<WindowDrag>) {
    for InputEvent(event) in messages.read() {
        let Some(press) = drag.press.as_mut() else {
            continue;
        };
        match event {
            Event::MouseDragged { point, .. } => press.pointer = Some(origin_from(*point)),
            Event::MouseUp {
                button: MouseButton::Left,
                ..
            }
            | Event::MouseMoved { .. } => press.released = true,
            _ => {}
        }
    }
}

/// Strips, for [`edge_scroll`] to move.
type ScrolledStrips<'w, 's> = Query<
    'w,
    's,
    (
        &'static LayoutStrip,
        &'static mut Position,
        Option<&'static mut RepositionMarker>,
        Has<Scrolling>,
        &'static ChildOf,
    ),
>;

/// Windows, for [`edge_scroll`] to move with their strip.
type ScrolledWindows<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Position,
        Option<&'static mut RepositionMarker>,
        &'static LayoutPosition,
        &'static Bounds,
        Option<&'static ResizeMarker>,
    ),
    (With<Window>, Without<LayoutStrip>),
>;

/// Scrolls the strip under a dragged window while the pointer is at its
/// display's edge, toward the columns beyond it, faster the nearer the edge.
/// The columns the window passes glide aside as they do without a scroll; the
/// others move with the strip, a frame at a time, as a swipe moves them.
fn edge_scroll(
    mut drag: ResMut<WindowDrag>,
    mut strips: ScrolledStrips,
    mut windows: ScrolledWindows,
    displays: Query<(&Display, Option<&DockPosition>)>,
    time: Res<Time>,
    config: Res<Config>,
) {
    let Some(press) = drag.press.as_mut() else {
        return;
    };
    let Phase::Dragging { held, .. } = &press.phase else {
        return;
    };
    let strip = strips
        .iter_mut()
        .find(|(strip, ..)| strip.contains(press.window));
    let at_edge = strip.and_then(|strip| {
        let (_, _, _, swiping, child_of) = &strip;
        let (display, dock) = displays.get(child_of.parent()).ok()?;
        let pointer = press.pointer.filter(|_| !press.released && !*swiping)?;
        let beyond = |point| {
            displays
                .iter()
                .any(|(display, _)| display.frame().contains(point))
        };
        let depth = edge_depth(pointer, display.frame(), beyond);
        (depth != 0.0).then_some((strip, depth, display, dock))
    });
    let Some(((strip, mut position, mut reposition, ..), depth, display, dock)) = at_edge else {
        if let Some(edge) = press.edge.take() {
            debug!("drag: the strip scrolled {} px at the edge", edge.scrolled);
        }
        return;
    };
    let edge = press.edge.get_or_insert_with(|| {
        debug!("drag: the strip scrolls at the edge");
        EdgeScroll::default()
    });

    let dt = time.delta_secs_f64();
    edge.held_for += dt;
    let ramp = (edge.held_for / EDGE_RAMP).min(1.0);
    let speed = EDGE_SPEED * f64::from(display.bounds().width()) * ramp;
    // Content follows the pointer's way in: at the right edge the strip goes
    // left, bringing in the columns past it.
    edge.carry -= depth * speed * dt;
    #[allow(clippy::cast_possible_truncation)]
    let whole = edge.carry.trunc() as i32;
    edge.carry -= f64::from(whole);

    // As far as the strip's far end coming in at the display's edge.
    let viewport = display.actual_display_bounds(dock, &config);
    let span = strip
        .last()
        .ok()
        .and_then(|column| column.top())
        .and_then(|entity| {
            let (_, _, layout, bounds, resize) = windows.get(entity).ok()?;
            Some(layout.0.x + resize.map_or(bounds.0, |resize| resize.0).x)
        });
    let strip_x = reposition.as_ref().map_or(position.0, |target| target.0).x;
    let step = match span {
        Some(span) if span > viewport.width() => {
            let (min, max) = (viewport.max.x - span, viewport.min.x);
            if whole < 0 && strip_x > min {
                (strip_x + whole).max(min) - strip_x
            } else if whole > 0 && strip_x < max {
                (strip_x + whole).min(max) - strip_x
            } else {
                0
            }
        }
        _ => 0,
    };
    if step != whole {
        edge.carry = 0.0;
    }
    if step == 0 {
        return;
    }
    edge.scrolled += step;

    position.0.x += step;
    if let Some(target) = reposition.as_mut() {
        target.0.x += step;
    }
    for entity in strip.all_windows() {
        if held.contains(&entity) {
            continue;
        }
        let Ok((mut position, reposition, layout, ..)) = windows.get_mut(entity) else {
            continue;
        };
        // Only a window where the layout has it, or gliding there: one
        // parked at the display's side stays put until it comes into view.
        let in_place = layout.0.x + strip_x;
        match reposition {
            Some(mut target) if target.0.x == in_place => {
                target.0.x += step;
                position.0.x += step;
            }
            None if position.0.x == in_place => position.0.x += step,
            _ => {}
        }
    }
}

/// How deep `pointer` is in the scrolling zone at a side of the display
/// `bounds`, from 0 outside it to 1 at the edge, positive on the right and
/// negative on the left; 0 at a side with a display `beyond` it.
fn edge_depth(pointer: Origin, bounds: IRect, beyond: impl Fn(Origin) -> bool) -> f64 {
    if !bounds.contains(pointer) {
        return 0.0;
    }
    let x = f64::from(pointer.x);
    // The last pixel is the edge: the pointer stops there. Bounds take in
    // their far side, which the display beyond starts on, so that is probed a
    // pixel further.
    let right = (x + 1.0 - (f64::from(bounds.max.x) - EDGE_ZONE)) / EDGE_ZONE;
    let left = (f64::from(bounds.min.x) + EDGE_ZONE - x) / EDGE_ZONE;
    if right > 0.0 && !beyond(Origin::new(bounds.max.x + 1, pointer.y)) {
        right.min(1.0)
    } else if left > 0.0 && !beyond(Origin::new(bounds.min.x - 1, pointer.y)) {
        -left.min(1.0)
    } else {
        0.0
    }
}

/// Columns of a strip as the layout sizes them, for [`nearest_slot`], and the
/// widths a carried column takes in another strip.
type WindowFrames<'w, 's> = Query<
    'w,
    's,
    (
        &'static Position,
        &'static mut Bounds,
        Option<&'static ResizeMarker>,
    ),
>;

/// Windows, with whether Paneru is moving or resizing them and whether one
/// has focus.
type DragWindows<'w, 's> = Query<
    'w,
    's,
    (
        &'static Window,
        Has<RepositionMarker>,
        Has<ResizeMarker>,
        Has<FocusedMarker>,
    ),
>;

/// Strips, with where they are heading, whether a swipe moves them, their
/// display, and whether it shows them.
type DragStrips<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut LayoutStrip,
        &'static Position,
        Option<&'static RepositionMarker>,
        Has<Scrolling>,
        &'static ChildOf,
        Has<SelectedVirtualMarker>,
        Has<NativeFullscreenMarker>,
    ),
>;

/// Turns a press into a drag once the window server moves the window with the
/// pointer, keeps the dragged column in the slot nearest the window, in the
/// strip of the display under the pointer, and lets the window glide into it
/// on release.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn drag_window(
    mut drag: ResMut<WindowDrag>,
    windows: DragWindows,
    mut strips: DragStrips,
    mut frames: WindowFrames,
    ratios: Query<&WidthRatio>,
    mut layout_positions: Query<&mut LayoutPosition>,
    displays: Query<(Entity, &Display, Option<&DockPosition>)>,
    window_manager: Res<WindowManager>,
    config: Res<Config>,
    mut commands: Commands,
) {
    let Some(press) = drag.press.as_mut() else {
        return;
    };
    let entity = press.window;
    let Ok((window, repositioning, resizing, focused)) = windows.get(entity) else {
        debug!("drag: window {entity} is gone");
        drag.press = None;
        return;
    };
    let current = strips
        .iter()
        .find_map(|(strip_entity, strip, ..)| strip.contains(entity).then_some(strip_entity));

    if matches!(press.phase, Phase::Pressed) {
        if press.released {
            drag.press = None;
            return;
        }
        let strip = current.and_then(|current| strips.get(current).ok());
        let strip_moving = strip
            .as_ref()
            .is_some_and(|(_, _, _, reposition, scrolling, ..)| reposition.is_some() || *scrolling);
        let frame = window.frame();
        if repositioning || resizing || strip_moving {
            // Paneru is moving it: that's no drag, and where it ends up is
            // where a drag would start from.
            press.frame = frame;
            return;
        }
        let moved_whole = frame.min != press.frame.min && frame.size() == press.frame.size();
        if press.pointer.is_none() || !moved_whole {
            return;
        }
        let Some((home, strip, position, reposition, ..)) = strip else {
            return;
        };
        let Ok(origin) = strip.index_of(entity) else {
            return;
        };
        if strip.is_fullscreen() {
            return;
        }
        let held = strip.tab_group(entity).unwrap_or_else(|| vec![entity]);
        let column = strip
            .get(origin)
            .map(|column| column.window_iter().collect())
            .unwrap_or_default();
        debug!(
            "drag: window {} from column {origin}, {} ms after the press",
            window.id(),
            press.at.elapsed().as_millis(),
        );
        for held in &held {
            if let Ok(mut entity_commands) = commands.get_entity(*held) {
                entity_commands.try_remove::<(RepositionMarker, VerifyWindowPosition)>();
            }
        }
        press.phase = Phase::Dragging {
            origin,
            held,
            home,
            column,
            over: None,
            away: false,
            scrolled_from: vec![(home, reposition.map_or(position.0, |target| target.0))],
        };
    }

    let Phase::Dragging {
        origin,
        held,
        home,
        column,
        over,
        away,
        scrolled_from,
    } = &mut press.phase
    else {
        return;
    };
    let Some(current) = current else {
        // The window left its strip: closed, minimized or sent elsewhere.
        debug!("drag: window {} left its strip", window.id());
        drag.press = None;
        return;
    };

    // The pointer's display, not the window's middle: a window held near
    // one side reaches past the display's edge before the pointer does. Its
    // menu bar is its own, on the way to a display above. The Space it shows
    // is asked once, as the pointer comes onto it.
    let under = press.pointer.and_then(|at| {
        displays
            .iter()
            .find(|(_, display, _)| display.frame().contains(at))
    });
    if over.map(|(display, _)| display) != under.map(|(display, ..)| display) {
        *over = under.map(|(display_entity, display, _)| {
            let space = window_manager.active_display_space(display.id()).ok();
            (display_entity, space)
        });
    }
    // The strip that display shows, unless it's full screen.
    let target = over.and_then(|(display, space)| {
        let space = space?;
        strips
            .iter()
            .filter(|(_, strip, _, _, _, child_of, ..)| {
                child_of.parent() == display && strip.id() == space
            })
            .min_by_key(|(_, strip, .., selected, _)| (!*selected, strip.virtual_index))
            .filter(|(_, strip, .., fullscreen)| !*fullscreen && !strip.is_fullscreen())
            .map(|(strip, ..)| strip)
    });
    let over_none = under.is_some() && target.is_none();
    if *away != over_none {
        *away = over_none;
        let over = if over_none { "no strip" } else { "a strip" };
        debug!("drag: window {} over {over}", window.id());
        if over_none {
            put_back(*home, scrolled_from, &strips, &mut commands);
        }
    }

    // Over no strip, the column goes back where it began.
    let frame = window.frame();
    let dest = target
        .or_else(|| strips.contains(*home).then_some(*home))
        .unwrap_or(current);
    let holder = if dest == current {
        let Ok((_, mut strip, position, reposition, ..)) = strips.get_mut(current) else {
            return;
        };
        let strip_x = reposition.map_or(position.0, |target| target.0).x;
        let slot = target.is_some().then_some((strip_x, frame.min.x));
        follow(&mut strip, entity, *origin, slot, &frames);
        current
    } else if let Ok([mut from, mut to]) = strips.get_many_mut([current, dest])
        && let Some((taken, tabbed)) = from.1.take_column(entity)
    {
        // As wide in its new strip as its share of that display, the held
        // window too, which takes that size as it's let go.
        if let Ok((_, display, dock)) = displays.get(to.5.parent()) {
            for member in taken.window_iter() {
                let ratio = ratios.get(member).map_or(0.5, |ratio| ratio.0);
                let width = width_on(ratio, display, dock, &config);
                if let Ok((_, mut bounds, _)) = frames.get_mut(member)
                    && width > 0
                    && bounds.0.x != width
                {
                    bounds.0.x = width;
                }
                if let Ok(mut entity_commands) = commands.get_entity(member) {
                    entity_commands.try_remove::<ResizeMarker>();
                }
            }
        }
        let heading = to.3.map_or(to.2.0, |target| target.0);
        if !scrolled_from.iter().any(|(strip, _)| *strip == dest) {
            scrolled_from.push((dest, heading));
        }
        let widths = column_widths(&to.1, None, &frames);
        let strip_x = heading.x;
        let slot = if target.is_some() {
            nearest_slot(&widths, strip_x, frame.min.x)
        } else {
            (*origin).min(widths.len())
        };
        to.1.insert_column(slot, taken, tabbed);
        match displays.get(to.5.parent()) {
            Ok((_, onto, _)) if target.is_some() => debug!(
                "drag: column to display {}'s strip, slot {slot} of {}",
                onto.id(),
                widths.len() + 1
            ),
            _ => debug!("drag: column back to its own strip"),
        }
        dest
    } else {
        current
    };
    if holder != current {
        put_back(current, scrolled_from, &strips, &mut commands);
    }

    let home_frame = strips
        .get(*home)
        .ok()
        .and_then(|(.., child_of, _, _)| displays.get(child_of.parent()).ok())
        .map(|(_, display, _)| display.frame());
    let left_away = *away && !home_frame.is_some_and(|home| mostly_on(frame, home));
    if press.released && left_away {
        // Its column stays where it began, and the window where it was let
        // go: that display's Space takes it, and Paneru goes by the Space.
        debug!("drag: window {} let go on another display", window.id());
        drag.press = None;
    } else if press.released {
        if *away {
            debug!(
                "drag: window {} let go over no strip, back to its slot",
                window.id()
            );
        } else {
            debug!("drag: window {} dropped", window.id());
        }
        let_land(held, &windows, &mut layout_positions, &mut frames);
        if holder != *home {
            // Until macOS gives the column the Space of the strip it joined,
            // it lists the windows on their old one.
            spawn_space_move_guard(std::mem::take(column), &mut commands);
            if focused && let Ok(mut entity_commands) = commands.get_entity(holder) {
                entity_commands.try_insert(ActiveWorkspaceMarker);
            }
        }
        if config.window_hidden_ratio() < 1.0 {
            commands.reshuffle_around(entity);
        }
        drag.press = None;
    }
}

/// Lets the held windows glide into their slot, at the size it has if
/// another display's strip took them or gave them back.
fn let_land(
    held: &[Entity],
    windows: &DragWindows,
    layout_positions: &mut Query<&mut LayoutPosition>,
    frames: &mut WindowFrames,
) {
    for held in held {
        if let Ok(mut layout_position) = layout_positions.get_mut(*held) {
            layout_position.set_changed();
        }
        if let Ok((_, mut bounds, _)) = frames.get_mut(*held)
            && windows
                .get(*held)
                .is_ok_and(|(window, ..)| window.frame().size() != bounds.0)
        {
            bounds.set_changed();
        }
    }
}

/// Sends `strip` back to where it was heading as the column came into it,
/// undoing the edge's scroll there.
fn put_back(
    strip: Entity,
    scrolled_from: &[(Entity, Origin)],
    strips: &DragStrips,
    commands: &mut Commands,
) {
    let Some((_, was)) = scrolled_from.iter().find(|(entity, _)| *entity == strip) else {
        return;
    };
    let Ok((_, _, position, reposition, ..)) = strips.get(strip) else {
        return;
    };
    if reposition.map_or(position.0, |target| target.0) != *was {
        debug!("drag: strip back where it was");
        commands.reposition_entity(strip, *was);
    }
}

/// Keeps `entity`'s column in the slot nearest `slot`'s window edge, given
/// the strip's own x, or in its first slot, `origin`, when there's none.
fn follow(
    strip: &mut LayoutStrip,
    entity: Entity,
    origin: usize,
    slot: Option<(i32, i32)>,
    frames: &WindowFrames,
) {
    let Ok(current) = strip.index_of(entity) else {
        return;
    };
    let others = column_widths(strip, Some(current), frames);
    let target = slot.map_or(origin.min(others.len()), |(strip_x, x)| {
        nearest_slot(&others, strip_x, x)
    });
    if target != current
        && let Some((column, tabbed)) = strip.take_column(entity)
    {
        debug!("drag: column {current} to {target} of {}", others.len() + 1);
        strip.insert_column(target, column, tabbed);
    }
}

/// Whether most of `frame` is on the display `bounds`.
fn mostly_on(frame: IRect, bounds: IRect) -> bool {
    let area = |rect: IRect| i64::from(rect.width()) * i64::from(rect.height());
    2 * area(frame.intersect(bounds)) >= area(frame)
}

/// The widths of the strip's columns, but for the one at `skip`.
fn column_widths(strip: &LayoutStrip, skip: Option<usize>, frames: &WindowFrames) -> Vec<i32> {
    strip
        .columns()
        .enumerate()
        .filter(|(index, _)| Some(*index) != skip)
        .map(|(_, column)| {
            column
                .width(&|entity| {
                    frames.get(entity).ok().map(|(position, bounds, resize)| {
                        let size = resize.map_or(bounds.0, |resize| resize.0);
                        IRect::from_corners(position.0, position.0 + size)
                    })
                })
                .unwrap_or_default()
        })
        .collect()
}

/// The slot, among the strip's other columns, whose left edge is nearest `x`:
/// past a neighbour's middle the dragged column goes the other side of it.
/// `widths` are the other columns' in order, and the strip starts at `strip_x`.
fn nearest_slot(widths: &[i32], strip_x: i32, x: i32) -> usize {
    let mut edge = strip_x;
    let mut nearest = (0, (edge - x).abs());
    for (index, width) in widths.iter().enumerate() {
        edge += width;
        let distance = (edge - x).abs();
        if distance < nearest.1 {
            nearest = (index + 1, distance);
        }
    }
    nearest.0
}

/// Where the dragged window will land, as subscribers are told: its slot in
/// the strip, where the strip is heading; while the pointer is over no strip,
/// its first slot in its own.
fn report_landing(
    mut drag: ResMut<WindowDrag>,
    windows: Query<(&Window, &LayoutPosition, &Bounds, Option<&ResizeMarker>)>,
    strips: Query<(&LayoutStrip, &Position, Option<&RepositionMarker>)>,
) {
    let landing = drag
        .press
        .as_ref()
        .filter(|press| matches!(press.phase, Phase::Dragging { .. }))
        .and_then(|press| {
            let (window, layout_position, bounds, resize) = windows.get(press.window).ok()?;
            let (_, position, reposition) = strips
                .iter()
                .find(|(strip, ..)| strip.contains(press.window))?;
            let origin = layout_position.0 + reposition.map_or(position.0, |target| target.0);
            let size = resize.map_or(bounds.0, |resize| resize.0);
            Some((window.id(), IRect::from_corners(origin, origin + size)))
        });
    if drag.landing != landing {
        drag.landing = landing;
    }
}

/// How wide a window whose width is `ratio` of its display's is on `display`.
/// `WidthRatio` is of a display's raw bounds, so it is applied to them, or the
/// share would shrink by the padding at each move; never wider than the
/// display shows.
fn width_on(ratio: f64, display: &Display, dock: Option<&DockPosition>, config: &Config) -> i32 {
    let most = display.actual_display_bounds(dock, config).width();
    round_px(ratio * f64::from(display.bounds().width())).min(most)
}

#[cfg(test)]
mod tests {
    use super::{edge_depth, nearest_slot};
    use crate::manager::Origin;
    use bevy::math::IRect;

    #[test]
    fn the_edge_zone_deepens_toward_the_edge() {
        let bounds = IRect::new(0, 0, 1024, 768);
        let alone = |_| false;
        let depth = |x| edge_depth(Origin::new(x, 300), bounds, alone);
        assert_eq!(depth(500), 0.0);
        assert_eq!(depth(975), 0.0);
        assert_eq!(depth(976), 1.0 / 48.0);
        assert_eq!(depth(1023), 1.0);
        assert_eq!(depth(47), -1.0 / 48.0);
        assert_eq!(depth(48), 0.0);
        assert_eq!(depth(0), -1.0);
        // Rounded up onto the display's far side.
        assert_eq!(depth(1024), 1.0);
        assert_eq!(depth(1030), 0.0);
    }

    #[test]
    fn a_side_with_a_display_beyond_it_doesnt_scroll() {
        let bounds = IRect::new(0, 0, 1024, 768);
        // As display bounds do, it takes in its near side.
        let right_of = |point: Origin| point.x >= 1024 && point.y <= 600;
        let depth = |y| edge_depth(Origin::new(1023, y), bounds, right_of);
        assert_eq!(depth(300), 0.0);
        // Below where the display beyond it ends.
        assert_eq!(depth(700), 1.0);
        assert_eq!(edge_depth(Origin::new(0, 300), bounds, right_of), -1.0);
    }

    #[test]
    fn a_column_passes_a_neighbour_at_its_middle() {
        // Two other columns, 400 and 800 wide, from x = 0: slots at 0, 400
        // and 1200.
        let widths = [400, 800];
        assert_eq!(nearest_slot(&widths, 0, -300), 0);
        assert_eq!(nearest_slot(&widths, 0, 199), 0);
        // A tie goes to the earlier slot.
        assert_eq!(nearest_slot(&widths, 0, 200), 0);
        assert_eq!(nearest_slot(&widths, 0, 201), 1);
        assert_eq!(nearest_slot(&widths, 0, 800), 1);
        assert_eq!(nearest_slot(&widths, 0, 801), 2);
        assert_eq!(nearest_slot(&widths, 0, 5000), 2);
    }

    #[test]
    fn slots_move_with_the_strip() {
        let widths = [400];
        assert_eq!(nearest_slot(&widths, -300, 0), 1);
        assert_eq!(nearest_slot(&widths, 300, 400), 0);
    }

    #[test]
    fn a_lone_column_has_one_slot() {
        assert_eq!(nearest_slot(&[], 0, 900), 0);
    }
}
