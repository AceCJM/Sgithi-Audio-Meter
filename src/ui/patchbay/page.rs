use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{
    gdk, Box as GtkBox, Button, DrawingArea, EventControllerMotion, EventControllerScroll, EventControllerScrollFlags,
    GestureClick, GestureDrag, Orientation, Popover, ScrolledWindow,
};

use crate::model::{Direction, Graph};
use crate::pw::Command;

use super::canvas_model::CanvasModel;
use super::persistence;
use super::render;

const MIN_ZOOM: f64 = 0.25;
const MAX_ZOOM: f64 = 3.0;
const ZOOM_STEP: f64 = 1.1;

/// What a left-button drag on the canvas is currently doing, decided by what was under the
/// pointer when the drag started.
pub enum DragState {
    None,
    MovingNode { id: u32, last_x: f64, last_y: f64 },
    DrawingLink { from_port: u32, cur_x: f64, cur_y: f64 },
}

pub struct PatchbayPage {
    pub widget: ScrolledWindow,
    drawing_area: DrawingArea,
    graph: Rc<RefCell<Graph>>,
    canvas: Rc<RefCell<CanvasModel>>,
    /// Canvas zoom factor - `render::draw` scales the whole `cairo::Context` by this before
    /// drawing, so every hit-testing/drag coordinate coming from a `DrawingArea` input event
    /// (always in real widget pixels) must be divided by it to get back to `CanvasModel`'s
    /// logical coordinate space. Changed via Ctrl+scroll - see `new()`.
    zoom: Rc<Cell<f64>>,
}

impl PatchbayPage {
    pub fn new(graph: Rc<RefCell<Graph>>, cmd_tx: pipewire::channel::Sender<Command>) -> Rc<Self> {
        let canvas = Rc::new(RefCell::new(CanvasModel::new(persistence::load(), persistence::load_hidden())));
        let drag = Rc::new(RefCell::new(DragState::None));
        let hover_link: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));
        let zoom = Rc::new(Cell::new(1.0_f64));

        let drawing_area = DrawingArea::new();
        drawing_area.set_content_width(900);
        drawing_area.set_content_height(700);

        {
            let graph = graph.clone();
            let canvas = canvas.clone();
            let drag = drag.clone();
            let hover_link = hover_link.clone();
            let zoom = zoom.clone();
            drawing_area.set_draw_func(move |_area, cr, _w, _h| {
                cr.scale(zoom.get(), zoom.get());
                render::draw(cr, &graph.borrow(), &canvas.borrow(), &drag.borrow(), hover_link.get());
            });
        }

        let drag_gesture = GestureDrag::new();
        {
            let graph = graph.clone();
            let canvas = canvas.clone();
            let drag = drag.clone();
            let drawing_area = drawing_area.clone();
            let zoom = zoom.clone();
            drag_gesture.connect_drag_begin(move |_gesture, x, y| {
                // Gesture coordinates are real widget pixels; `CanvasModel` and `DragState` work
                // in the unzoomed logical space `render::draw` scales up from - see `zoom`'s doc
                // comment on `PatchbayPage`.
                let (lx, ly) = (x / zoom.get(), y / zoom.get());
                let g = graph.borrow();
                let c = canvas.borrow();
                let mut state = drag.borrow_mut();
                *state = if let Some(port_id) = c.port_at(&g, lx, ly) {
                    DragState::DrawingLink { from_port: port_id, cur_x: lx, cur_y: ly }
                } else if let Some(node_id) = c.node_at(&g, lx, ly) {
                    // Kept in raw widget pixels (not logical) since it's only ever differenced
                    // against another raw widget-pixel point in `connect_drag_update` below.
                    DragState::MovingNode { id: node_id, last_x: x, last_y: y }
                } else {
                    DragState::None
                };
                drop(state);
                drop(c);
                drop(g);
                drawing_area.queue_draw();
            });
        }
        {
            let canvas = canvas.clone();
            let drag = drag.clone();
            let drawing_area = drawing_area.clone();
            let zoom = zoom.clone();
            drag_gesture.connect_drag_update(move |gesture, offset_x, offset_y| {
                let (start_x, start_y) = gesture.start_point().unwrap_or((0.0, 0.0));
                let mut state = drag.borrow_mut();
                match &mut *state {
                    DragState::MovingNode { id, last_x, last_y } => {
                        let (cur_x, cur_y) = (start_x + offset_x, start_y + offset_y);
                        let z = zoom.get();
                        canvas.borrow_mut().move_node(*id, (cur_x - *last_x) / z, (cur_y - *last_y) / z);
                        *last_x = cur_x;
                        *last_y = cur_y;
                    }
                    DragState::DrawingLink { cur_x, cur_y, .. } => {
                        let z = zoom.get();
                        *cur_x = (start_x + offset_x) / z;
                        *cur_y = (start_y + offset_y) / z;
                    }
                    DragState::None => {}
                }
                drop(state);
                drawing_area.queue_draw();
            });
        }
        {
            let graph = graph.clone();
            let canvas = canvas.clone();
            let drag = drag.clone();
            let drawing_area = drawing_area.clone();
            let cmd_tx = cmd_tx.clone();
            let zoom = zoom.clone();
            drag_gesture.connect_drag_end(move |gesture, offset_x, offset_y| {
                let (start_x, start_y) = gesture.start_point().unwrap_or((0.0, 0.0));
                let mut state = drag.borrow_mut();
                match &*state {
                    DragState::DrawingLink { from_port, .. } => {
                        let z = zoom.get();
                        let (end_x, end_y) = ((start_x + offset_x) / z, (start_y + offset_y) / z);
                        let g = graph.borrow();
                        let c = canvas.borrow();
                        if let Some(to_port) = c.port_at(&g, end_x, end_y) {
                            try_create_link(&g, &cmd_tx, *from_port, to_port);
                        }
                    }
                    DragState::MovingNode { .. } => {
                        persistence::save(&canvas.borrow().positions_by_name(&graph.borrow()));
                    }
                    DragState::None => {}
                }
                *state = DragState::None;
                drop(state);
                drawing_area.queue_draw();
            });
        }
        drawing_area.add_controller(drag_gesture);

        // Right-click: on a node, a Hide menu; on a link, remove it immediately (as before); on
        // empty canvas, a menu to bring back any currently-hidden nodes.
        let click_gesture = GestureClick::new();
        click_gesture.set_button(3);
        {
            let graph = graph.clone();
            let canvas = canvas.clone();
            let drawing_area = drawing_area.clone();
            let cmd_tx = cmd_tx.clone();
            let zoom = zoom.clone();
            click_gesture.connect_pressed(move |_gesture, _n, x, y| {
                // Hit-testing needs logical coordinates, but the popover is positioned in the
                // `DrawingArea`'s own (real widget-pixel) space, so `x, y` themselves stay as-is.
                let (lx, ly) = (x / zoom.get(), y / zoom.get());
                let hit = {
                    let g = graph.borrow();
                    let c = canvas.borrow();
                    c.node_at(&g, lx, ly).map(Hit::Node).or_else(|| c.link_at(&g, lx, ly).map(Hit::Link))
                };
                match hit {
                    Some(Hit::Node(node_id)) => show_node_menu(&drawing_area, x, y, node_id, &canvas, &graph),
                    Some(Hit::Link(link_id)) => {
                        let _ = cmd_tx.send(Command::DestroyLink { link_id });
                    }
                    None => show_hidden_nodes_menu(&drawing_area, x, y, &canvas, &graph),
                }
                drawing_area.queue_draw();
            });
        }
        drawing_area.add_controller(click_gesture);

        // Highlight the link under the pointer, so it's clear what a right-click would remove.
        let motion = EventControllerMotion::new();
        {
            let graph = graph.clone();
            let canvas = canvas.clone();
            let hover_link = hover_link.clone();
            let drawing_area = drawing_area.clone();
            let zoom = zoom.clone();
            motion.connect_motion(move |_c, x, y| {
                let (lx, ly) = (x / zoom.get(), y / zoom.get());
                let found = canvas.borrow().link_at(&graph.borrow(), lx, ly);
                if found != hover_link.get() {
                    hover_link.set(found);
                    drawing_area.queue_draw();
                }
            });
        }
        drawing_area.add_controller(motion);

        // Ctrl+scroll zooms the canvas; a plain scroll is left alone (Propagation::Proceed) so
        // the ScrolledWindow's own scrollbars keep panning normally.
        let scroll = EventControllerScroll::new(EventControllerScrollFlags::VERTICAL);
        {
            let graph = graph.clone();
            let canvas = canvas.clone();
            let drawing_area = drawing_area.clone();
            let zoom = zoom.clone();
            scroll.connect_scroll(move |controller, _dx, dy| {
                if !controller.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                    return glib::Propagation::Proceed;
                }
                let factor = if dy < 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                let new_zoom = (zoom.get() * factor).clamp(MIN_ZOOM, MAX_ZOOM);
                zoom.set(new_zoom);
                let (w, h) = content_size(&graph.borrow(), &canvas.borrow(), new_zoom);
                drawing_area.set_content_width(w);
                drawing_area.set_content_height(h);
                drawing_area.queue_draw();
                glib::Propagation::Stop
            });
        }
        drawing_area.add_controller(scroll);

        let widget = ScrolledWindow::new();
        widget.set_child(Some(&drawing_area));
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        Rc::new(Self { widget, drawing_area, graph, canvas, zoom })
    }

    /// Re-layout for any new/removed nodes and repaint. Called after every graph-changing event.
    pub fn sync(&self) {
        let (w, h) = {
            let g = self.graph.borrow();
            let mut c = self.canvas.borrow_mut();
            c.sync(&g);
            content_size(&g, &c, self.zoom.get())
        };
        self.drawing_area.set_content_width(w);
        self.drawing_area.set_content_height(h);
        self.drawing_area.queue_draw();
    }
}

/// The `DrawingArea`'s required content size for a given zoom level - the logical extent
/// (`CanvasModel::extent`, already floored to a sane minimum) scaled up the same way
/// `render::draw` scales the `cairo::Context`, so the `ScrolledWindow`'s scrollbars match what's
/// actually drawn.
fn content_size(graph: &Graph, canvas: &CanvasModel, zoom: f64) -> (i32, i32) {
    let (w, h) = canvas.extent(graph);
    ((w.max(900.0) * zoom) as i32, (h.max(700.0) * zoom) as i32)
}

enum Hit {
    Node(u32),
    Link(u32),
}

/// A small popover, anchored to the click point, with one action button per row. Used for both
/// the node context menu and the hidden-nodes menu below.
fn build_menu_popover(parent: &impl IsA<gtk::Widget>, x: f64, y: f64, rows: Vec<(String, Rc<dyn Fn()>)>) {
    let content = GtkBox::new(Orientation::Vertical, 2);
    content.set_margin_top(4);
    content.set_margin_bottom(4);
    content.set_margin_start(4);
    content.set_margin_end(4);

    let popover = Popover::new();
    popover.set_has_arrow(false);
    popover.set_autohide(true);
    popover.set_parent(parent);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

    if rows.is_empty() {
        content.append(&gtk::Label::new(Some("No hidden nodes")));
    }
    for (label, action) in rows {
        let button = Button::with_label(&label);
        button.add_css_class("flat");
        {
            let popover = popover.clone();
            button.connect_clicked(move |_| {
                action();
                popover.popdown();
            });
        }
        content.append(&button);
    }

    popover.set_child(Some(&content));
    popover.popup();
}

fn show_node_menu(
    drawing_area: &DrawingArea,
    x: f64,
    y: f64,
    node_id: u32,
    canvas: &Rc<RefCell<CanvasModel>>,
    graph: &Rc<RefCell<Graph>>,
) {
    let hide = {
        let canvas = canvas.clone();
        let graph = graph.clone();
        let drawing_area = drawing_area.clone();
        Rc::new(move || {
            canvas.borrow_mut().hide(node_id);
            persistence::save_hidden(&canvas.borrow().hidden_by_name(&graph.borrow()));
            drawing_area.queue_draw();
        }) as Rc<dyn Fn()>
    };
    build_menu_popover(drawing_area, x, y, vec![("Hide".to_string(), hide)]);
}

fn show_hidden_nodes_menu(
    drawing_area: &DrawingArea,
    x: f64,
    y: f64,
    canvas: &Rc<RefCell<CanvasModel>>,
    graph: &Rc<RefCell<Graph>>,
) {
    let g = graph.borrow();
    let rows: Vec<(String, Rc<dyn Fn()>)> = canvas
        .borrow()
        .hidden_nodes(&g)
        .into_iter()
        .map(|(id, name)| {
            let label = format!("Show: {name}");
            let canvas = canvas.clone();
            let graph = graph.clone();
            let drawing_area = drawing_area.clone();
            let action = Rc::new(move || {
                canvas.borrow_mut().show(id);
                persistence::save_hidden(&canvas.borrow().hidden_by_name(&graph.borrow()));
                drawing_area.queue_draw();
            }) as Rc<dyn Fn()>;
            (label, action)
        })
        .collect();
    drop(g);
    build_menu_popover(drawing_area, x, y, rows);
}

fn try_create_link(graph: &Graph, cmd_tx: &pipewire::channel::Sender<Command>, a: u32, b: u32) {
    let (Some(port_a), Some(port_b)) = (graph.ports.get(&a), graph.ports.get(&b)) else {
        return;
    };
    let (output, input) = match (port_a.direction, port_b.direction) {
        (Direction::Output, Direction::Input) => (port_a, port_b),
        (Direction::Input, Direction::Output) => (port_b, port_a),
        _ => return,
    };
    if output.node_id == input.node_id {
        return;
    }
    let _ = cmd_tx.send(Command::CreateLink {
        output_node: output.node_id,
        output_port: output.id,
        input_node: input.node_id,
        input_port: input.id,
    });
}
