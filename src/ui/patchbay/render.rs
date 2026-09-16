use gtk::cairo;

use crate::model::{Direction, Graph};

use super::canvas_model::{bezier_control_points, CanvasModel, HEADER_HEIGHT, NODE_WIDTH, PORT_RADIUS, PORT_ROW_HEIGHT};
use super::page::DragState;

pub fn draw(cr: &cairo::Context, graph: &Graph, canvas: &CanvasModel, drag: &DragState, hover_link: Option<u32>) {
    cr.set_source_rgb(0.13, 0.13, 0.15);
    let _ = cr.paint();

    for link in graph.links.values() {
        draw_link(cr, graph, canvas, link.output_port, link.input_port, hover_link == Some(link.id));
    }

    if let DragState::DrawingLink { from_port, cur_x, cur_y } = drag {
        if let Some(port) = graph.ports.get(from_port) {
            if let Some(from) = canvas.port_pos(graph, port) {
                let to = (*cur_x, *cur_y);
                let (p1, p2) = bezier_control_points(from, to);
                cr.set_source_rgb(0.55, 0.75, 1.0);
                cr.set_line_width(2.0);
                cr.move_to(from.0, from.1);
                cr.curve_to(p1.0, p1.1, p2.0, p2.1, to.0, to.1);
                let _ = cr.stroke();
            }
        }
    }

    let mut ids: Vec<_> = graph.nodes.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        draw_node(cr, graph, canvas, id);
    }
}

fn draw_link(cr: &cairo::Context, graph: &Graph, canvas: &CanvasModel, output_port: u32, input_port: u32, highlight: bool) {
    let (Some(out_port), Some(in_port)) = (graph.ports.get(&output_port), graph.ports.get(&input_port)) else {
        return;
    };
    let (Some(from), Some(to)) = (canvas.port_pos(graph, out_port), canvas.port_pos(graph, in_port)) else {
        return;
    };
    let (p1, p2) = bezier_control_points(from, to);

    if highlight {
        cr.set_source_rgb(1.0, 0.4, 0.4);
        cr.set_line_width(3.0);
    } else {
        cr.set_source_rgb(0.6, 0.6, 0.65);
        cr.set_line_width(2.0);
    }
    cr.move_to(from.0, from.1);
    cr.curve_to(p1.0, p1.1, p2.0, p2.1, to.0, to.1);
    let _ = cr.stroke();
}

fn draw_node(cr: &cairo::Context, graph: &Graph, canvas: &CanvasModel, id: u32) {
    let Some(node) = graph.nodes.get(&id) else { return };
    let Some((x, y, w, h)) = canvas.node_rect(graph, id) else { return };

    rounded_rect(cr, x, y, w, h, 6.0);
    cr.set_source_rgb(0.19, 0.19, 0.22);
    let _ = cr.fill_preserve();
    cr.set_source_rgb(0.35, 0.35, 0.4);
    cr.set_line_width(1.0);
    let _ = cr.stroke();

    cr.set_source_rgb(0.9, 0.9, 0.92);
    cr.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    cr.set_font_size(12.0);
    cr.move_to(x + 8.0, y + HEADER_HEIGHT - 8.0);
    let _ = cr.show_text(&truncate(node.display_name(), 22));

    cr.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(10.0);

    for port in graph.ports_for_node(id) {
        let Some((px, py)) = canvas.port_pos(graph, port) else { continue };

        cr.set_source_rgb(0.75, 0.75, 0.78);
        // Both directions' labels are left-aligned just inside the node body; right-aligning
        // input labels against the port circle would need text-extent measurement, which isn't
        // worth the extra cairo API surface for a v1 label.
        let label_x = if port.direction == Direction::Output { px + 8.0 } else { x + 8.0 };
        cr.move_to(label_x, py + 3.0);
        let _ = cr.show_text(&truncate(&port.name, 14));

        cr.arc(px, py, PORT_RADIUS, 0.0, std::f64::consts::TAU);
        cr.set_source_rgb(0.55, 0.8, 0.55);
        let _ = cr.fill();
    }

    let _ = w;
    let _ = h;
    let _ = PORT_ROW_HEIGHT;
    let _ = NODE_WIDTH;
}

fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        t.push('\u{2026}');
        t
    }
}
