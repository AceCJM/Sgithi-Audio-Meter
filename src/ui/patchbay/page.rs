use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{DrawingArea, EventControllerMotion, GestureClick, GestureDrag, ScrolledWindow};

use crate::model::{Direction, Graph};
use crate::pw::Command;

use super::canvas_model::CanvasModel;
use super::render;

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
}

impl PatchbayPage {
    pub fn new(graph: Rc<RefCell<Graph>>, cmd_tx: pipewire::channel::Sender<Command>) -> Rc<Self> {
        let canvas = Rc::new(RefCell::new(CanvasModel::new()));
        let drag = Rc::new(RefCell::new(DragState::None));
        let hover_link: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));

        let drawing_area = DrawingArea::new();
        drawing_area.set_content_width(900);
        drawing_area.set_content_height(700);

        {
            let graph = graph.clone();
            let canvas = canvas.clone();
            let drag = drag.clone();
            let hover_link = hover_link.clone();
            drawing_area.set_draw_func(move |_area, cr, _w, _h| {
                render::draw(cr, &graph.borrow(), &canvas.borrow(), &drag.borrow(), hover_link.get());
            });
        }

        let drag_gesture = GestureDrag::new();
        {
            let graph = graph.clone();
            let canvas = canvas.clone();
            let drag = drag.clone();
            let drawing_area = drawing_area.clone();
            drag_gesture.connect_drag_begin(move |_gesture, x, y| {
                let g = graph.borrow();
                let c = canvas.borrow();
                let mut state = drag.borrow_mut();
                *state = if let Some(port_id) = c.port_at(&g, x, y) {
                    DragState::DrawingLink { from_port: port_id, cur_x: x, cur_y: y }
                } else if let Some(node_id) = c.node_at(&g, x, y) {
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
            drag_gesture.connect_drag_update(move |gesture, offset_x, offset_y| {
                let (start_x, start_y) = gesture.start_point().unwrap_or((0.0, 0.0));
                let mut state = drag.borrow_mut();
                match &mut *state {
                    DragState::MovingNode { id, last_x, last_y } => {
                        let (cur_x, cur_y) = (start_x + offset_x, start_y + offset_y);
                        canvas.borrow_mut().move_node(*id, cur_x - *last_x, cur_y - *last_y);
                        *last_x = cur_x;
                        *last_y = cur_y;
                    }
                    DragState::DrawingLink { cur_x, cur_y, .. } => {
                        *cur_x = start_x + offset_x;
                        *cur_y = start_y + offset_y;
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
            drag_gesture.connect_drag_end(move |gesture, offset_x, offset_y| {
                let (start_x, start_y) = gesture.start_point().unwrap_or((0.0, 0.0));
                let mut state = drag.borrow_mut();
                if let DragState::DrawingLink { from_port, .. } = &*state {
                    let (end_x, end_y) = (start_x + offset_x, start_y + offset_y);
                    let g = graph.borrow();
                    let c = canvas.borrow();
                    if let Some(to_port) = c.port_at(&g, end_x, end_y) {
                        try_create_link(&g, &cmd_tx, *from_port, to_port);
                    }
                }
                *state = DragState::None;
                drop(state);
                drawing_area.queue_draw();
            });
        }
        drawing_area.add_controller(drag_gesture);

        let click_gesture = GestureClick::new();
        click_gesture.set_button(3); // secondary/right click removes a link
        {
            let graph = graph.clone();
            let canvas = canvas.clone();
            let drawing_area = drawing_area.clone();
            let cmd_tx = cmd_tx.clone();
            click_gesture.connect_pressed(move |_gesture, _n, x, y| {
                let link_id = {
                    let g = graph.borrow();
                    let c = canvas.borrow();
                    c.link_at(&g, x, y)
                };
                if let Some(link_id) = link_id {
                    let _ = cmd_tx.send(Command::DestroyLink { link_id });
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
            motion.connect_motion(move |_c, x, y| {
                let found = canvas.borrow().link_at(&graph.borrow(), x, y);
                if found != hover_link.get() {
                    hover_link.set(found);
                    drawing_area.queue_draw();
                }
            });
        }
        drawing_area.add_controller(motion);

        let widget = ScrolledWindow::new();
        widget.set_child(Some(&drawing_area));
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        Rc::new(Self { widget, drawing_area, graph, canvas })
    }

    /// Re-layout for any new/removed nodes and repaint. Called after every graph-changing event.
    pub fn sync(&self) {
        let (w, h) = {
            let g = self.graph.borrow();
            let mut c = self.canvas.borrow_mut();
            c.sync(&g);
            c.extent(&g)
        };
        self.drawing_area.set_content_width(w.max(900.0) as i32);
        self.drawing_area.set_content_height(h.max(700.0) as i32);
        self.drawing_area.queue_draw();
    }
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
