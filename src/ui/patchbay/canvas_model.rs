use std::collections::HashMap;

use crate::model::{Direction, Graph, PortInfo};

pub const NODE_WIDTH: f64 = 180.0;
pub const HEADER_HEIGHT: f64 = 26.0;
pub const PORT_ROW_HEIGHT: f64 = 18.0;
pub const PORT_RADIUS: f64 = 5.0;

const COLUMN_GAP: f64 = 60.0;
const ROW_GAP: f64 = 24.0;
const MARGIN: f64 = 20.0;

pub struct NodeLayout {
    pub x: f64,
    pub y: f64,
}

/// Screen-space layout for the patchbay canvas: a position per node (auto-placed in columns by
/// direction on first appearance, user-draggable afterward) plus the hit-testing/coordinate math
/// shared between rendering and interaction.
pub struct CanvasModel {
    positions: HashMap<u32, NodeLayout>,
}

impl CanvasModel {
    pub fn new() -> Self {
        Self { positions: HashMap::new() }
    }

    /// Drop layout entries for nodes that no longer exist, and place any new nodes.
    pub fn sync(&mut self, graph: &Graph) {
        self.positions.retain(|id, _| graph.nodes.contains_key(id));

        let mut ids: Vec<_> = graph.nodes.keys().copied().collect();
        ids.sort_unstable();

        let mut column_y = [MARGIN, MARGIN, MARGIN];
        for id in ids {
            if self.positions.contains_key(&id) {
                continue;
            }
            let has_out = graph.ports_for_node(id).any(|p| p.direction == Direction::Output);
            let has_in = graph.ports_for_node(id).any(|p| p.direction == Direction::Input);
            let column = if has_out && !has_in {
                0
            } else if has_in && !has_out {
                2
            } else {
                1
            };
            let x = MARGIN + column as f64 * (NODE_WIDTH + COLUMN_GAP);
            let y = column_y[column];
            column_y[column] += self.node_height(graph, id) + ROW_GAP;
            self.positions.insert(id, NodeLayout { x, y });
        }
    }

    pub fn node_height(&self, graph: &Graph, id: u32) -> f64 {
        let in_count = graph.ports_for_node(id).filter(|p| p.direction == Direction::Input).count();
        let out_count = graph.ports_for_node(id).filter(|p| p.direction == Direction::Output).count();
        HEADER_HEIGHT + in_count.max(out_count).max(1) as f64 * PORT_ROW_HEIGHT + 10.0
    }

    pub fn node_rect(&self, graph: &Graph, id: u32) -> Option<(f64, f64, f64, f64)> {
        let pos = self.positions.get(&id)?;
        Some((pos.x, pos.y, NODE_WIDTH, self.node_height(graph, id)))
    }

    pub fn port_pos(&self, graph: &Graph, port: &PortInfo) -> Option<(f64, f64)> {
        let (x, y, w, _h) = self.node_rect(graph, port.node_id)?;
        let mut index = 0usize;
        let mut count = 0usize;
        for p in graph.ports_for_node(port.node_id) {
            if p.direction == port.direction {
                if p.id == port.id {
                    index = count;
                }
                count += 1;
            }
        }
        let py = y + HEADER_HEIGHT + PORT_ROW_HEIGHT * index as f64 + PORT_ROW_HEIGHT / 2.0;
        let px = if port.direction == Direction::Input { x } else { x + w };
        Some((px, py))
    }

    pub fn node_at(&self, graph: &Graph, x: f64, y: f64) -> Option<u32> {
        self.positions.keys().copied().find(|&id| {
            self.node_rect(graph, id)
                .map(|(nx, ny, nw, nh)| x >= nx && x <= nx + nw && y >= ny && y <= ny + nh)
                .unwrap_or(false)
        })
    }

    pub fn port_at(&self, graph: &Graph, x: f64, y: f64) -> Option<u32> {
        graph
            .ports
            .values()
            .find(|port| {
                self.port_pos(graph, port)
                    .map(|(px, py)| {
                        let (dx, dy) = (px - x, py - y);
                        (dx * dx + dy * dy).sqrt() <= PORT_RADIUS + 5.0
                    })
                    .unwrap_or(false)
            })
            .map(|p| p.id)
    }

    /// The link nearest to `(x, y)` within a small click tolerance, found by sampling each
    /// link's bezier curve (cheap enough for the node counts a real PipeWire graph has).
    pub fn link_at(&self, graph: &Graph, x: f64, y: f64) -> Option<u32> {
        const SAMPLES: usize = 24;
        const TOLERANCE: f64 = 6.0;

        let mut best: Option<(u32, f64)> = None;
        for link in graph.links.values() {
            let (Some(out_port), Some(in_port)) =
                (graph.ports.get(&link.output_port), graph.ports.get(&link.input_port))
            else {
                continue;
            };
            let (Some(p0), Some(p3)) = (self.port_pos(graph, out_port), self.port_pos(graph, in_port)) else {
                continue;
            };
            let (p1, p2) = bezier_control_points(p0, p3);

            for i in 0..=SAMPLES {
                let t = i as f64 / SAMPLES as f64;
                let (bx, by) = cubic_bezier(p0, p1, p2, p3, t);
                let dist = ((bx - x).powi(2) + (by - y).powi(2)).sqrt();
                if dist <= TOLERANCE && best.map(|(_, d)| dist < d).unwrap_or(true) {
                    best = Some((link.id, dist));
                }
            }
        }
        best.map(|(id, _)| id)
    }

    pub fn move_node(&mut self, id: u32, dx: f64, dy: f64) {
        if let Some(p) = self.positions.get_mut(&id) {
            p.x += dx;
            p.y += dy;
        }
    }

    pub fn extent(&self, graph: &Graph) -> (f64, f64) {
        let mut w = 800.0_f64;
        let mut h = 600.0_f64;
        for &id in self.positions.keys() {
            if let Some((x, y, nw, nh)) = self.node_rect(graph, id) {
                w = w.max(x + nw + MARGIN);
                h = h.max(y + nh + MARGIN);
            }
        }
        (w, h)
    }
}

pub fn bezier_control_points((x0, y0): (f64, f64), (x3, y3): (f64, f64)) -> ((f64, f64), (f64, f64)) {
    let dx = ((x3 - x0) / 2.0).abs().max(40.0);
    ((x0 + dx, y0), (x3 - dx, y3))
}

pub fn cubic_bezier(
    (x0, y0): (f64, f64),
    (x1, y1): (f64, f64),
    (x2, y2): (f64, f64),
    (x3, y3): (f64, f64),
    t: f64,
) -> (f64, f64) {
    let mt = 1.0 - t;
    let x = mt * mt * mt * x0 + 3.0 * mt * mt * t * x1 + 3.0 * mt * t * t * x2 + t * t * t * x3;
    let y = mt * mt * mt * y0 + 3.0 * mt * mt * t * y1 + 3.0 * mt * t * t * y2 + t * t * t * y3;
    (x, y)
}
