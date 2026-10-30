//! Graph canvas, layout sim thread and mouse interaction.

use super::*;

/// Camera + interaction state shared with the canvas paint closures.
#[derive(Default)]
pub(super) struct GraphViewport {
    bounds: Bounds<Pixels>,
    cam: [f32; 2],
    zoom: f32,
    centered: bool,
    center_on: Option<usize>,
    hover: Option<usize>,
    zoom_target: f32,
    pub(super) query: Option<std::collections::HashSet<String>>,
    bounds_origin: [f32; 2],
}
#[derive(Default)]
pub(super) struct GraphPaint {
    nodes: Vec<(usize, [f32; 2], bool, bool)>,
    edges: Vec<([f32; 2], [f32; 2])>,
    labels: Vec<(usize, [f32; 2])>,
    /// Hover focus: neighbours of the hovered node (others dim).
    focus: Option<std::collections::HashSet<usize>>,
}
/// Node colour: the shared per-tag colour (`style::tag_color`, the
/// same one the editor chips use), so a tag looks identical everywhere.
pub(super) fn graph_node_color(colors: &RowColors, node: &crate::graph::GNode) -> Hsla {
    if node.is_tag {
        return colors.muted_foreground;
    }
    match &node.first_tag {
        Some(tag) => style::tag_color(tag, colors.background),
        None => colors.blue,
    }
}
pub(super) struct GraphView {
    pub(super) data: std::sync::Arc<crate::graph::GraphData>,
    pos: std::sync::Arc<std::sync::Mutex<std::sync::Arc<Vec<[f32; 2]>>>>,
    pinned: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<usize>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    settled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(super) vp: std::rc::Rc<std::cell::RefCell<GraphViewport>>,
    labels: std::rc::Rc<std::cell::RefCell<(String, std::collections::HashMap<usize, ShapedLine>)>>,
    grid: std::collections::HashMap<(i32, i32), Vec<u32>>,
    grid_stamp: (usize, i32, i32, u32),
    snap: usize,
    drag_node: Option<usize>,
    drag_moved: bool,
    down_pos: [f32; 2],
    pan: Option<([f32; 2], [f32; 2])>,
    enlarged: bool,
    prev_size: Option<(f32, f32)>,
}

impl LazyKasten {
    pub(super) fn stop_graph(&mut self) {
        if let Some(g) = &self.graph {
            g.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.graph_seq += 1;
    }
    pub fn open_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Editor {
            self.save(window, cx);
        }
        self.g_center = self.note_id.clone();
        self.g_local = self.g_center.is_some();
        // The search box is shared with the browser/palette: the graph
        // starts with an empty query, not the last screen's text.
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.load_graph(window, cx);
    }
    pub(super) fn load_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_graph();
        let seq = self.graph_seq;
        self.graph = None;
        self.graph_loading = true;
        self.screen = Screen::Graph;
        self.graph_focus.focus(window, cx);
        cx.notify();
        let store = self.store.clone();
        let mode = if self.g_local { crate::graph::GraphMode::Local } else { crate::graph::GraphMode::Global };
        let center = self.g_center.clone();
        let (depth, tags_on, orphans) = (self.g_depth, self.g_tags, self.g_orphans);
        let dir = self.store.data_dir();
        let task = cx.background_spawn(async move {
            let data = crate::graph::build(&store, mode, center.as_deref(), depth, tags_on, orphans).ok()?;
            let cache = if mode == crate::graph::GraphMode::Global {
                crate::graph::load_cache(&dir)
            } else {
                std::collections::HashMap::new()
            };
            let layout = crate::graph::Layout::new(&data, &cache);
            Some((data, layout.pos))
        });
        cx.spawn(async move |this, cx| {
            let loaded = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.graph_seq != seq {
                    return;
                }
                this.graph_loading = false;
                if let Some((data, pos)) = loaded {
                    let center_idx = this
                        .g_center
                        .as_ref()
                        .and_then(|id| data.by_id.get(id).copied());
                    let mut vp = GraphViewport { zoom: 1.0, zoom_target: 1.0, ..Default::default() };
                    vp.center_on = center_idx.or(if data.nodes.is_empty() { None } else { Some(0) });
                    this.graph = Some(GraphView {
                        data: std::sync::Arc::new(data),
                        pos: std::sync::Arc::new(std::sync::Mutex::new(std::sync::Arc::new(pos))),
                        pinned: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
                        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                        running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                        settled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                        vp: std::rc::Rc::new(std::cell::RefCell::new(vp)),
                        labels: std::rc::Rc::new(std::cell::RefCell::new((String::new(), std::collections::HashMap::new()))),
                        grid: std::collections::HashMap::new(),
                        grid_stamp: (usize::MAX, 0, 0, 0),
                        snap: 0,
                        drag_node: None,
                        drag_moved: false,
                        down_pos: [0.0, 0.0],
                        pan: None,
                        enlarged: false,
                        prev_size: None,
                    });
                    this.start_sim(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn start_sim(&mut self, cx: &mut Context<Self>) {
        let Some(g) = &self.graph else { return };
        if g.running.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        g.settled.store(false, std::sync::atomic::Ordering::SeqCst);
        let seq = self.graph_seq;
        let data = g.data.clone();
        let pos = g.pos.clone();
        let pinned = g.pinned.clone();
        let stop = g.stop.clone();
        let running = g.running.clone();
        let settled = g.settled.clone();
        let settled_ui = settled.clone();
        let is_global = !self.g_local;
        let dir = self.store.data_dir();
        let (tx, rx) = async_channel::unbounded::<()>();
        std::thread::spawn(move || {
            let initial = (**pos.lock().unwrap()).clone();
            let mut layout = crate::graph::Layout::from_positions(&data, initial);
            for _ in 0..3000 {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let pins: std::collections::HashSet<usize> = pinned.lock().unwrap().clone();
                {
                    let shared = pos.lock().unwrap();
                    for &i in &pins {
                        if i < layout.pos.len() {
                            layout.pos[i] = shared[i];
                        }
                    }
                }
                let max_move = layout.step(&data, &pins);
                *pos.lock().unwrap() = std::sync::Arc::new(layout.pos.clone());
                if max_move < 0.08 {
                    settled.store(true, std::sync::atomic::Ordering::SeqCst);
                    if is_global {
                        crate::graph::save_cache(&dir, &data, &layout.pos);
                    }
                    let _ = tx.try_send(());
                    break;
                }
                let _ = tx.try_send(());
                std::thread::sleep(std::time::Duration::from_millis(8));
            }
            running.store(false, std::sync::atomic::Ordering::SeqCst);
        });
        cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                let mut done = false;
                let _ = this.update(cx, |this, cx| {
                    if this.graph_seq == seq {
                        if let Some(g) = &mut this.graph {
                            g.snap += 1;
                        }
                        if settled_ui.load(std::sync::atomic::Ordering::SeqCst) {
                            done = true;
                            // Re-center once the layout settles (nodes
                            // drift from their hash start positions).
                            if let Some(g) = &this.graph {
                                let idx = if this.g_local {
                                    this.g_center.as_ref().and_then(|id| g.data.by_id.get(id).copied())
                                } else {
                                    Some(0)
                                };
                                if let Some(idx) = idx {
                                    if !g.data.nodes.is_empty() {
                                        g.vp.borrow_mut().center_on = Some(idx);
                                    }
                                }
                            }
                        }
                        cx.notify();
                    } else {
                        done = true;
                    }
                });
                if done {
                    break;
                }
            }
        })
        .detach();
    }
    pub(super) fn graph_hit(&mut self, local: [f32; 2]) -> Option<usize> {
        let g = self.graph.as_mut()?;
        let (cam, zoom) = {
            let vp = g.vp.borrow();
            (vp.cam, vp.zoom)
        };
        let world = [(local[0] - cam[0]) / zoom, (local[1] - cam[1]) / zoom];
        let stamp = (g.snap, cam[0] as i32, cam[1] as i32, zoom.to_bits());
        if g.grid_stamp != stamp {
            let pos = g.pos.lock().unwrap();
            g.grid = crate::graph::build_grid(&pos, 24.0);
            g.grid_stamp = stamp;
        }
        let pos = g.pos.lock().unwrap();
        crate::graph::nearest(&g.grid, &pos, world, 12.0 / zoom, 24.0)
    }
    pub(super) fn graph_local(&self, ev_pos: Point<Pixels>) -> [f32; 2] {
        let Some(g) = &self.graph else { return [0.0, 0.0] };
        let vp = g.vp.borrow();
        [f32::from(ev_pos.x) - vp.bounds_origin[0], f32::from(ev_pos.y) - vp.bounds_origin[1]]
    }
    /// Vim tokens on the graph canvas: hjkl pan, e/E zoom, i opens the
    /// node nearest the centre, q leaves.
    pub(super) fn on_graph_vim_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.vim_enabled || self.screen != Screen::Graph {
            return;
        }
        if self.search.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        let Some(kc) = &ev.keystroke.key_char else { return };
        let Some(c) = kc.chars().next() else { return };
        let Some(g) = &self.graph else { return };
        match c {
            'h' | 'j' | 'k' | 'l' => {
                let mut vp = g.vp.borrow_mut();
                match c {
                    'h' => vp.cam[0] += 40.0,
                    'l' => vp.cam[0] -= 40.0,
                    'j' => vp.cam[1] -= 40.0,
                    _ => vp.cam[1] += 40.0,
                }
            }
            'e' => {
                let mut vp = g.vp.borrow_mut();
                vp.zoom_target = (vp.zoom_target * 1.15).min(3.0);
            }
            'E' => {
                let mut vp = g.vp.borrow_mut();
                vp.zoom_target = (vp.zoom_target / 1.15).max(0.15);
            }
            'i' => {
                // Open the note node nearest the canvas centre.
                let vp = g.vp.borrow();
                let (w, h) = (
                    f32::from(vp.bounds.size.width),
                    f32::from(vp.bounds.size.height),
                );
                let center = [(w / 2.0 - vp.cam[0]) / vp.zoom, (h / 2.0 - vp.cam[1]) / vp.zoom];
                drop(vp);
                let pos = g.pos.lock().unwrap();
                let mut best: Option<(usize, f32)> = None;
                for (i, node) in g.data.nodes.iter().enumerate() {
                    if node.is_tag {
                        continue;
                    }
                    let d = (pos[i][0] - center[0]).powi(2) + (pos[i][1] - center[1]).powi(2);
                    if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                        best = Some((i, d));
                    }
                }
                drop(pos);
                if let Some((i, _)) = best {
                    let id = g.data.nodes[i].id.clone();
                    self.stop_graph();
                    self.return_to_graph = true;
                    self.open_note(&id, window, cx);
                }
            }
            'q' => {
                self.leave_graph(window, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn graph_mouse_down(&mut self, ev: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if ev.button != MouseButton::Left || self.graph.is_none() {
            return;
        }
        let local = self.graph_local(ev.position);
        let hit = self.graph_hit(local);
        match hit {
            Some(i) => {
                if let Some(g) = &mut self.graph {
                    g.drag_node = Some(i);
                    g.drag_moved = false;
                    g.down_pos = local;
                    g.pinned.lock().unwrap().insert(i);
                }
                self.start_sim(cx);
            }
            None => {
                if let Some(g) = &mut self.graph {
                    let cam = g.vp.borrow().cam;
                    g.pan = Some((local, cam));
                }
            }
        }
        cx.notify();
    }
    pub(super) fn graph_mouse_move(&mut self, ev: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let local = self.graph_local(ev.position);
        enum Act {
            Drag(usize, [f32; 2]),
            Pan([f32; 2]),
            Hover,
        }
        let act = {
            let Some(g) = &self.graph else { return };
            let vp = g.vp.borrow();
            let world = [(local[0] - vp.cam[0]) / vp.zoom, (local[1] - vp.cam[1]) / vp.zoom];
            if let Some(i) = g.drag_node {
                Act::Drag(i, world)
            } else if let Some((start, cam0)) = g.pan {
                Act::Pan([cam0[0] + local[0] - start[0], cam0[1] + local[1] - start[1]])
            } else {
                Act::Hover
            }
        };
        match act {
            Act::Drag(i, world) => {
                if let Some(g) = &mut self.graph {
                    let dx = local[0] - g.down_pos[0];
                    let dy = local[1] - g.down_pos[1];
                    if dx * dx + dy * dy >= 16.0 {
                        g.drag_moved = true;
                    }
                    let mut guard = g.pos.lock().unwrap();
                    std::sync::Arc::make_mut(&mut *guard)[i] = world;
                    g.snap += 1;
                }
                cx.notify();
            }
            Act::Pan(cam) => {
                if let Some(g) = &self.graph {
                    g.vp.borrow_mut().cam = cam;
                }
                cx.notify();
            }
            Act::Hover => {
                let hit = self.graph_hit(local);
                if let Some(g) = &mut self.graph {
                    let changed = g.vp.borrow().hover != hit;
                    if changed {
                        g.vp.borrow_mut().hover = hit;
                        cx.notify();
                    }
                }
            }
        }
    }
    pub(super) fn graph_mouse_up(&mut self, _ev: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let mut center_on: Option<usize> = None;
        let mut open_id: Option<String> = None;
        if let Some(g) = &mut self.graph {
            if let Some(i) = g.drag_node.take() {
                g.pinned.lock().unwrap().remove(&i);
                if !g.drag_moved {
                    let node = &g.data.nodes[i];
                    if node.is_tag {
                        center_on = Some(i);
                    } else {
                        open_id = Some(node.id.clone());
                    }
                }
            }
            g.pan = None;
        }
        if let Some(id) = open_id {
            self.stop_graph();
            self.return_to_graph = true;
            self.open_note(&id, window, cx);
            return;
        }
        if let (Some(i), Some(g)) = (center_on, &self.graph) {
            let pos = g.pos.lock().unwrap();
            let p = pos[i];
            let mut vp = g.vp.borrow_mut();
            let (w, h) = (f32::from(vp.bounds.size.width), f32::from(vp.bounds.size.height));
            vp.cam = [w / 2.0 - p[0] * vp.zoom, h / 2.0 - p[1] * vp.zoom];
        }
        cx.notify();
    }
    pub(super) fn graph_wheel(&mut self, ev: &ScrollEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.graph.is_none() {
            return;
        }
        let local = self.graph_local(ev.position);
        let delta = ev.delta.pixel_delta(px(16.0));
        if let Some(g) = &self.graph {
            let mut vp = g.vp.borrow_mut();
            // Smooth zoom: the wheel moves the target; frames lerp to it
            // (instant when LAZYKASTEN_REDUCED_MOTION=1).
            let factor = (1.0 - f32::from(delta.y) / 300.0).clamp(0.7, 1.4);
            vp.zoom_target = (vp.zoom_target * factor).clamp(0.15, 3.0);
            if std::env::var_os("LAZYKASTEN_REDUCED_MOTION").is_some() {
                let world = [(local[0] - vp.cam[0]) / vp.zoom, (local[1] - vp.cam[1]) / vp.zoom];
                vp.zoom = vp.zoom_target;
                vp.cam = [local[0] - world[0] * vp.zoom, local[1] - world[1] * vp.zoom];
            }
        }
        cx.notify();
    }
    pub(super) fn leave_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_graph();
        self.restore_graph_size(window);
        self.screen = Screen::Editor;
        self.editor.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }
    pub(super) fn restore_graph_size(&mut self, window: &mut Window) {
        if let Some(g) = &mut self.graph {
            if g.enlarged {
                if let Some((w, h)) = g.prev_size {
                    window.resize(size(px(w), px(h)));
                }
                g.enlarged = false;
            }
        }
    }
    pub(super) fn toggle_graph_enlarge(&mut self, window: &mut Window) {
        let Some(g) = &mut self.graph else { return };
        if g.enlarged {
            self.restore_graph_size(window);
            return;
        }
        let viewport = window.viewport_size();
        g.prev_size = Some((f32::from(viewport.width), f32::from(viewport.height)));
        // Current-monitor work area via Win32, converted to logical pixels.
        let scale = window.scale_factor().max(1.0);
        let (mut w, mut h) = (f32::from(viewport.width), f32::from(viewport.height));
        unsafe {
            use windows::Win32::Foundation::RECT;
            use windows::Win32::UI::WindowsAndMessaging::{
                SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
            };
            let mut rc = RECT::default();
            if SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut rc as *mut _ as *mut std::ffi::c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
            .is_ok()
            {
                w = (rc.right - rc.left) as f32 / scale;
                h = (rc.bottom - rc.top) as f32 / scale;
            }
        }
        window.resize(size(px(w), px(h)));
        g.enlarged = true;
    }
    pub(super) fn render_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = row_colors(cx);
        if let Some(g) = &self.graph {
            let zooming = {
                let v = g.vp.borrow();
                (v.zoom_target - v.zoom).abs() > 0.001
            };
            if !g.settled.load(std::sync::atomic::Ordering::SeqCst) || zooming {
                window.request_animation_frame();
            }
        }
        let status = if self.graph_loading {
            "Loading graph\u{2026}".to_string()
        } else if let Some(g) = &self.graph {
            let settled = g.settled.load(std::sync::atomic::Ordering::SeqCst);
            format!(
                "{} nodes \u{00B7} {} edges \u{00B7} {}{}{}",
                g.data.nodes.len(),
                g.data.edges.len(),
                if self.g_local { "Local" } else { "Global" },
                if settled { "" } else { " \u{00B7} settling\u{2026}" },
                match &self.g_center {
                    Some(c) if self.g_local => format!(" \u{00B7} center {c}"),
                    _ => String::new(),
                },
            )
        } else {
            "No graph".to_string()
        };

        // Legend: the six biggest first-tags with their shared colours.
        let legend: Option<AnyElement> = self.graph.as_ref().map(|g| {
            let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
            for n in &g.data.nodes {
                if let Some(t) = &n.first_tag {
                    *counts.entry(t.as_str()).or_default() += 1;
                }
            }
            let mut top: Vec<(&str, usize)> = counts.into_iter().collect();
            top.sort_by(|a, b| b.1.cmp(&a.1));
            top.truncate(6);
            let mut el = div()
                .absolute()
                .left(px(12.0))
                .bottom(px(12.0))
                .flex()
                .flex_col()
                .gap_1()
                .px(px(style::tokens::SPACE_2))
                .py(px(style::tokens::SPACE_2))
                .rounded(px(style::tokens::RADIUS_CARD))
                .border_1()
                .border_color(colors.border)
                .bg(colors.background)
                .text_size(px(style::tokens::FONT_MUTED));
            for (tag, n) in top {
                let c = style::tag_color(tag, colors.background);
                el = el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(10.0)).h(px(10.0)).rounded_full().bg(c))
                        .child(div().text_color(colors.muted_foreground).child(format!("#{tag} ({n})"))),
                );
            }
            el.into_any_element()
        });

        let canvas_el: AnyElement = if let Some(g) = &self.graph {
            let data = g.data.clone();
            let pos_arc = g.pos.clone();
            let vp = g.vp.clone();
            let labels = g.labels.clone();
            let hover = g.vp.borrow().hover;
            let query = g.vp.borrow().query.clone();
            canvas(
                {
                    let data = data.clone();
                    let pos_arc = pos_arc.clone();
                    let vp = vp.clone();
                    move |bounds, _window, _cx| {
                        let pos = pos_arc.lock().unwrap();
                        let mut v = vp.borrow_mut();
                        v.bounds = bounds;
                        v.bounds_origin = [f32::from(bounds.origin.x), f32::from(bounds.origin.y)];
                        if !v.centered {
                            v.cam = [f32::from(bounds.size.width) / 2.0, f32::from(bounds.size.height) / 2.0];
                            v.centered = true;
                        }
                        if let Some(idx) = v.center_on.take() {
                            if let Some(p) = pos.get(idx) {
                                v.cam = [
                                    f32::from(bounds.size.width) / 2.0 - p[0] * v.zoom,
                                    f32::from(bounds.size.height) / 2.0 - p[1] * v.zoom,
                                ];
                            }
                        }
                        // Smooth-zoom step: lerp toward the target,
                        // keeping the canvas-centre world point fixed.
                        if (v.zoom_target - v.zoom).abs() > 0.001 {
                            let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                            let world_c = [(bw / 2.0 - v.cam[0]) / v.zoom, (bh / 2.0 - v.cam[1]) / v.zoom];
                            v.zoom += (v.zoom_target - v.zoom) * 0.3;
                            if (v.zoom_target - v.zoom).abs() <= 0.002 {
                                v.zoom = v.zoom_target;
                            }
                            v.cam = [bw / 2.0 - world_c[0] * v.zoom, bh / 2.0 - world_c[1] * v.zoom];
                        }
                        let (cam, zoom) = (v.cam, v.zoom);
                        drop(v);
                        let to_screen = |p: [f32; 2]| -> [f32; 2] {
                            [f32::from(bounds.origin.x) + p[0] * zoom + cam[0], f32::from(bounds.origin.y) + p[1] * zoom + cam[1]]
                        };
                        let in_view = |s: [f32; 2], m: f32| {
                            s[0] > f32::from(bounds.origin.x) - m
                                && s[0] < f32::from(bounds.origin.x) + f32::from(bounds.size.width) + m
                                && s[1] > f32::from(bounds.origin.y) - m
                                && s[1] < f32::from(bounds.origin.y) + f32::from(bounds.size.height) + m
                        };
                        let mut paint = GraphPaint::default();
                        for (i, node) in data.nodes.iter().enumerate() {
                            let s = to_screen(pos[i]);
                            if in_view(s, 40.0) {
                                let dim = query.as_ref().map(|q| !node.is_tag && !q.contains(&node.id)).unwrap_or(false);
                                paint.nodes.push((i, s, node.is_tag, dim));
                            }
                        }
                        if data.nodes.len() <= 4000 {
                            for (a, b) in &data.edges {
                                let (sa, sb) = (to_screen(pos[*a as usize]), to_screen(pos[*b as usize]));
                                if in_view(sa, 0.0) || in_view(sb, 0.0) {
                                    paint.edges.push((sa, sb));
                                }
                            }
                        }
                        // Hover focus set: hovered node + its neighbours.
                        if let Some(h) = hover {
                            let mut set = std::collections::HashSet::new();
                            set.insert(h);
                            for (a, b) in &data.edges {
                                if *a as usize == h {
                                    set.insert(*b as usize);
                                } else if *b as usize == h {
                                    set.insert(*a as usize);
                                }
                            }
                            paint.focus = Some(set);
                        }
                        // Labels: all when zoomed in, else the 150
                        // highest-degree visible nodes (plus the hover).
                        let mut label_idx: Vec<usize> = paint.nodes.iter().map(|(i, _, _, _)| *i).collect();
                        if zoom <= 0.8 {
                            label_idx.sort_by_key(|&i| std::cmp::Reverse(data.degree[i]));
                            label_idx.truncate(150);
                        }
                        if let Some(h) = hover {
                            if paint.nodes.iter().any(|(i, _, _, _)| *i == h) && !label_idx.contains(&h) {
                                label_idx.push(h);
                            }
                        }
                        for (i, s, _, _) in &paint.nodes {
                            if label_idx.contains(i) {
                                paint.labels.push((*i, *s));
                            }
                        }
                        paint
                    }
                },
                move |bounds, paint, window, cx| {
                    let theme_name = cx.theme().theme_name().to_string();
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        // Edges: one stroke path for everything visible.
                        let mut edge_color = colors.muted_foreground;
                        edge_color.a = 0.35;
                        let mut path = PathBuilder::stroke(px(1.0));
                        for (a, b) in &paint.edges {
                            path.move_to(point(px(a[0]), px(a[1])));
                            path.line_to(point(px(b[0]), px(b[1])));
                        }
                        if let Ok(p) = path.build() {
                            window.paint_path(p, edge_color);
                        }
                        // Nodes.
                        for (i, s, is_tag, dim) in &paint.nodes {
                            let node = &data.nodes[*i];
                            let r = if *is_tag {
                                5.0
                            } else {
                                (3.0 + (data.degree[*i] as f32).sqrt() * 1.1).min(10.0)
                            };
                            let mut color = graph_node_color(&colors, node);
                            if *dim {
                                color.a *= 0.15;
                            }
                            if let Some(focus) = &paint.focus {
                                if !focus.contains(i) {
                                    color.a *= 0.25;
                                }
                            }
                            let rect = Bounds::new(
                                point(px(s[0] - r), px(s[1] - r)),
                                size(px(r * 2.0), px(r * 2.0)),
                            );
                            if *is_tag {
                                window.paint_quad(fill(rect, color));
                            } else {
                                window.paint_quad(fill(rect, color).corner_radii(px(r)));
                            }
                            if hover == Some(*i) {
                                let mut ring = colors.foreground;
                                ring.a = 0.9;
                                let rect2 = Bounds::new(
                                    point(px(s[0] - r - 3.0), px(s[1] - r - 3.0)),
                                    size(px((r + 3.0) * 2.0), px((r + 3.0) * 2.0)),
                                );
                                window.paint_quad(fill(rect2, ring).corner_radii(px(r + 3.0)));
                                window.paint_quad(fill(rect, color).corner_radii(px(r)));
                            }
                        }
                        // Labels (shaped lines cached per node and theme).
                        let font = window.text_style().font();
                        let mut cache = labels.borrow_mut();
                        if cache.0 != theme_name {
                            cache.1.clear();
                            cache.0 = theme_name.clone();
                        }
                        for (i, s) in &paint.labels {
                            let node = &data.nodes[*i];
                            let shaped = cache.1.entry(*i).or_insert_with(|| {
                                window.text_system().shape_line(
                                    node.label.clone().into(),
                                    px(11.0),
                                    &[TextRun {
                                        len: node.label.len(),
                                        font: font.clone(),
                                        color: colors.foreground,
                                        background_color: None,
                                        underline: None,
                                        strikethrough: None,
                                    }],
                                    None,
                                )
                            });
                            let _ = shaped.paint(
                                point(px(s[0] + 8.0), px(s[1] - 6.0)),
                                px(14.0),
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        }
                    });
                },
            )
            .size_full()
            .into_any_element()
        } else {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.muted_foreground)
                .child(if self.graph_loading { "Loading graph\u{2026}" } else { "No graph" })
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .px_6()
                    .pt_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(Input::new(&self.search).appearance(true))
                    .child(div().text_color(colors.muted_foreground).child(status)),
            )
            .child(
                div()
                    .flex_1()
                    .relative()
                    .track_focus(&self.graph_focus)
                    .key_context("GraphView")
                    .capture_key_down(cx.listener(Self::on_graph_vim_key))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::graph_mouse_down))
                    .on_mouse_move(cx.listener(Self::graph_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::graph_mouse_up))
                    .on_scroll(cx.listener(Self::graph_wheel))
                    .child(canvas_el)
                    .children(legend),
            )
            .child(
                div()
                    .h_8()
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(colors.border)
                    .px_4()
                    .text_color(colors.muted_foreground)
                    .child("Tab Local/Global   T tag nodes   O orphans   1/2/3 depth   / search   Ctrl+Enter enlarge   drag nodes   wheel zoom   click opens   Esc back"),
            )
    }
}
