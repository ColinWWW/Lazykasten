//! Graph view data and layout: notes/tags loaded from the index in a few
//! queries, positions by a Barnes-Hut force layout (deterministic per
//! vault), cached to `<vault>/.lazykasten/graph_layout.bin`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use anyhow::Result;

use crate::store::Store;

pub const TAG_CAP: usize = 2000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GraphMode {
    Local,
    Global,
}

#[derive(Clone, Debug)]
pub struct GNode {
    /// SQLite rowid for notes, 0 for tag nodes.
    pub rowid: i64,
    /// Note id, or the tag name for tag nodes.
    pub id: String,
    pub label: String,
    pub is_tag: bool,
    pub first_tag: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct GraphData {
    pub nodes: Vec<GNode>,
    pub edges: Vec<(u32, u32)>,
    /// Whether each edge is a tag membership (weaker spring).
    pub edge_is_tag: Vec<bool>,
    pub degree: Vec<u32>,
    #[allow(dead_code)] // used by cache tests
    pub by_rowid: HashMap<i64, usize>,
    pub by_id: HashMap<String, usize>,
}

pub fn build(
    store: &Store,
    mode: GraphMode,
    center_id: Option<&str>,
    depth: usize,
    tags_on: bool,
    orphans: bool,
) -> Result<GraphData> {
    build_with_cap(store, mode, center_id, depth, tags_on, orphans, TAG_CAP)
}

pub fn build_with_cap(
    store: &Store,
    mode: GraphMode,
    center_id: Option<&str>,
    depth: usize,
    tags_on: bool,
    orphans: bool,
    tag_cap: usize,
) -> Result<GraphData> {
    let notes = store.graph_notes()?;
    let links = store.graph_links()?;
    let tag_groups = if tags_on { store.graph_tag_notes()? } else { Vec::new() };

    // Row index over all notes (before any filtering).
    let all: Vec<GNode> = notes
        .into_iter()
        .map(|(rowid, id, title, first_tag)| GNode {
            rowid,
            id,
            label: title,
            is_tag: false,
            first_tag,
        })
        .collect();
    let all_by_rowid: HashMap<i64, usize> =
        all.iter().enumerate().map(|(i, n)| (n.rowid, i)).collect();

    let mut link_edges: Vec<(usize, usize)> = Vec::new();
    let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();
    for (a, b) in &links {
        if let (Some(&ia), Some(&ib)) = (all_by_rowid.get(a), all_by_rowid.get(b)) {
            if ia != ib {
                link_edges.push((ia, ib));
                adjacency.entry(ia).or_default().push(ib);
                adjacency.entry(ib).or_default().push(ia);
            }
        }
    }
    let mut degree_all = vec![0u32; all.len()];
    for (a, b) in &link_edges {
        degree_all[*a] += 1;
        degree_all[*b] += 1;
    }

    // Which notes are included?
    let mut included: HashSet<usize> = HashSet::new();
    match mode {
        GraphMode::Global => {
            for i in 0..all.len() {
                if degree_all[i] > 0 || orphans {
                    included.insert(i);
                }
            }
        }
        GraphMode::Local => {
            if let Some(center) = center_id.and_then(|id| all.iter().position(|n| n.id == id)) {
                included.insert(center);
                let mut queue = VecDeque::from([(center, 0usize)]);
                let mut seen = HashSet::from([center]);
                while let Some((node, d)) = queue.pop_front() {
                    if d >= depth {
                        continue;
                    }
                    if let Some(next) = adjacency.get(&node) {
                        for &n in next {
                            if seen.insert(n) {
                                included.insert(n);
                                queue.push_back((n, d + 1));
                            }
                        }
                    }
                }
                // Tag-sharing neighbors join the local graph when tag
                // nodes are on (one extra ring, not expanded further).
                if tags_on {
                    let center_tags: HashSet<&str> = tag_groups
                        .iter()
                        .filter(|(_, ids)| ids.iter().any(|r| all_by_rowid.get(r) == Some(&center)))
                        .map(|(t, _)| t.as_str())
                        .collect();
                    for (tag, ids) in &tag_groups {
                        if center_tags.contains(tag.as_str()) {
                            for r in ids {
                                if let Some(&i) = all_by_rowid.get(r) {
                                    included.insert(i);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Remap included notes to compact indices, sorted by rowid (all is
    // already rowid-ordered, so a filtered pass keeps determinism).
    let mut remap: HashMap<usize, usize> = HashMap::new();
    let mut nodes: Vec<GNode> = Vec::new();
    for (i, node) in all.iter().enumerate() {
        if included.contains(&i) {
            remap.insert(i, nodes.len());
            nodes.push(node.clone());
        }
    }
    let mut edges: Vec<(u32, u32)> = Vec::new();
    let mut edge_is_tag: Vec<bool> = Vec::new();
    let mut edge_set: HashSet<(u32, u32)> = HashSet::new();
    for (a, b) in &link_edges {
        if let (Some(&ia), Some(&ib)) = (remap.get(a), remap.get(b)) {
            let key = (ia.min(ib) as u32, ia.max(ib) as u32);
            if edge_set.insert(key) {
                edges.push(key);
                edge_is_tag.push(false);
            }
        }
    }

    // Tag nodes (skipped entirely when more than `tag_cap` notes carry the
    // tag: such a node would swallow the screen).
    if tags_on {
        for (tag, ids) in &tag_groups {
            if ids.len() > tag_cap {
                continue;
            }
            let members: Vec<usize> = ids
                .iter()
                .filter_map(|r| all_by_rowid.get(r))
                .filter_map(|i| remap.get(i).copied())
                .collect();
            if members.is_empty() {
                continue;
            }
            let tag_idx = nodes.len();
            nodes.push(GNode {
                rowid: 0,
                id: tag.clone(),
                label: format!("#{tag}"),
                is_tag: true,
                first_tag: None,
            });
            for m in members {
                let key = (tag_idx.min(m) as u32, tag_idx.max(m) as u32);
                if edge_set.insert(key) {
                    edges.push(key);
                    edge_is_tag.push(true);
                }
            }
        }
    }

    let mut degree = vec![0u32; nodes.len()];
    for (a, b) in &edges {
        degree[*a as usize] += 1;
        degree[*b as usize] += 1;
    }
    let by_rowid: HashMap<i64, usize> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| !n.is_tag)
        .map(|(i, n)| (n.rowid, i))
        .collect();
    let by_id: HashMap<String, usize> =
        nodes.iter().enumerate().map(|(i, n)| (n.id.clone(), i)).collect();
    Ok(GraphData { nodes, edges, edge_is_tag, degree, by_rowid, by_id })
}

// -- layout ---------------------------------------------------------------

/// Deterministic pseudo-random start position in a disc, from the id.
fn hash_position(seed: &str) -> [f32; 2] {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in seed.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let angle = (h % 6283) as f32 / 1000.0;
    let radius = 40.0 + ((h >> 16) % 360) as f32;
    [radius * angle.cos(), radius * angle.sin()]
}

pub struct Layout {
    pub pos: Vec<[f32; 2]>,
    vel: Vec<[f32; 2]>,
}

impl Layout {
    pub fn new(data: &GraphData, cache: &HashMap<i64, [f32; 2]>) -> Self {
        let pos = data
            .nodes
            .iter()
            .map(|n| {
                if n.is_tag {
                    hash_position(&format!("tag:{}", n.id))
                } else {
                    cache.get(&n.rowid).copied().unwrap_or_else(|| hash_position(&n.id))
                }
            })
            .collect();
        Layout { pos, vel: vec![[0.0, 0.0]; data.nodes.len()] }
    }

    pub fn from_positions(data: &GraphData, pos: Vec<[f32; 2]>) -> Self {
        let n = data.nodes.len();
        Layout { pos, vel: vec![[0.0, 0.0]; n] }
    }

    /// One step; returns the largest movement (settle signal).
    pub fn step(&mut self, data: &GraphData, pinned: &HashSet<usize>) -> f32 {
        let forces = compute_forces(data, &self.pos, true);
        let mut max_move = 0.0f32;
        for i in 0..self.pos.len() {
            if pinned.contains(&i) {
                self.vel[i] = [0.0, 0.0];
                continue;
            }
            let v = &mut self.vel[i];
            v[0] = (v[0] + forces[i][0]) * 0.85;
            v[1] = (v[1] + forces[i][1]) * 0.85;
            let speed = (v[0] * v[0] + v[1] * v[1]).sqrt();
            if speed > 12.0 {
                v[0] *= 12.0 / speed;
                v[1] *= 12.0 / speed;
            }
            self.pos[i][0] += v[0];
            self.pos[i][1] += v[1];
            max_move = max_move.max(speed.min(12.0));
        }
        max_move
    }
}

const REPULSION: f32 = 900.0;
const THETA: f32 = 0.8;

pub fn compute_forces(data: &GraphData, pos: &[[f32; 2]], barnes_hut: bool) -> Vec<[f32; 2]> {
    let mut forces = vec![[0.0f32; 2]; pos.len()];
    if barnes_hut {
        repulsion_barnes_hut(pos, &mut forces);
    } else {
        repulsion_brute(pos, &mut forces);
    }
    // Springs.
    for (ei, (a, b)) in data.edges.iter().enumerate() {
        let (a, b) = (*a as usize, *b as usize);
        let (dx, dy) = (pos[b][0] - pos[a][0], pos[b][1] - pos[a][1]);
        let dist = (dx * dx + dy * dy).sqrt().max(1.0);
        let (rest, k) = if data.edge_is_tag[ei] { (46.0, 0.012) } else { (64.0, 0.03) };
        let f = (dist - rest) * k / dist;
        forces[a][0] += dx * f;
        forces[a][1] += dy * f;
        forces[b][0] -= dx * f;
        forces[b][1] -= dy * f;
    }
    // Gentle gravity to the origin keeps components together.
    for (i, f) in forces.iter_mut().enumerate() {
        f[0] += -pos[i][0] * 0.008;
        f[1] += -pos[i][1] * 0.008;
    }
    forces
}

pub fn repulsion_brute(pos: &[[f32; 2]], forces: &mut [[f32; 2]]) {
    for i in 0..pos.len() {
        for j in (i + 1)..pos.len() {
            let (dx, dy) = (pos[i][0] - pos[j][0], pos[i][1] - pos[j][1]);
            let d2 = (dx * dx + dy * dy).max(4.0);
            let dist = d2.sqrt();
            let f = REPULSION / d2 / dist;
            forces[i][0] += dx * f;
            forces[i][1] += dy * f;
            forces[j][0] -= dx * f;
            forces[j][1] -= dy * f;
        }
    }
}

struct TreeNode {
    center: [f32; 2],
    size: f32,
    mass: f32,
    com: [f32; 2],
    children: [i32; 4],
    point: i32,
}

fn repulsion_barnes_hut(pos: &[[f32; 2]], forces: &mut [[f32; 2]]) {
    if pos.is_empty() {
        return;
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pos {
        minx = minx.min(p[0]);
        miny = miny.min(p[1]);
        maxx = maxx.max(p[0]);
        maxy = maxy.max(p[1]);
    }
    let size = ((maxx - minx).max(maxy - miny)).max(1.0);
    let mut tree: Vec<TreeNode> = Vec::with_capacity(pos.len() * 2);
    tree.push(TreeNode {
        center: [(minx + maxx) / 2.0, (miny + maxy) / 2.0],
        size,
        mass: 0.0,
        com: [0.0, 0.0],
        children: [-1; 4],
        point: -1,
    });
    for (i, p) in pos.iter().enumerate() {
        insert(&mut tree, 0, i, *p, pos, 0);
    }
    for i in 0..pos.len() {
        apply_repulsion(&tree, 0, i, pos[i], forces);
    }
}

fn insert(tree: &mut Vec<TreeNode>, node: usize, point_idx: usize, p: [f32; 2], pos: &[[f32; 2]], depth: u32) {
    // Accumulate mass/center of mass.
    {
        let n = &mut tree[node];
        let m = n.mass;
        n.com[0] = (n.com[0] * m + p[0]) / (m + 1.0);
        n.com[1] = (n.com[1] * m + p[1]) / (m + 1.0);
        n.mass += 1.0;
    }
    if depth > 24 || tree[node].size < 0.5 {
        return;
    }
    if tree[node].point == -1 && tree[node].children.iter().all(|c| *c == -1) && tree[node].mass <= 1.0 {
        tree[node].point = point_idx as i32;
        return;
    }
    // Subdivide: move an existing point down first.
    let existing = tree[node].point;
    if existing >= 0 {
        tree[node].point = -1;
        let existing_pos = pos[existing as usize];
        insert_into_child(tree, node, existing as usize, existing_pos, pos, depth);
    }
    insert_into_child(tree, node, point_idx, p, pos, depth);
}

fn insert_into_child(tree: &mut Vec<TreeNode>, node: usize, point_idx: usize, p: [f32; 2], pos: &[[f32; 2]], depth: u32) {
    let (center, size) = (tree[node].center, tree[node].size);
    let quad = (if p[0] >= center[0] { 1 } else { 0 }) + (if p[1] >= center[1] { 2 } else { 0 });
    let child = tree[node].children[quad];
    if child == -1 {
        let half = size / 2.0;
        let quarter = size / 4.0;
        let cc = [
            center[0] + if quad & 1 == 1 { quarter } else { -quarter },
            center[1] + if quad & 2 == 2 { quarter } else { -quarter },
        ];
        tree.push(TreeNode {
            center: cc,
            size: half,
            mass: 0.0,
            com: [0.0, 0.0],
            children: [-1; 4],
            point: -1,
        });
        let new_child = (tree.len() - 1) as i32;
        tree[node].children[quad] = new_child;
        insert(tree, new_child as usize, point_idx, p, pos, depth + 1);
    } else {
        insert(tree, child as usize, point_idx, p, pos, depth + 1);
    }
}

fn apply_repulsion(tree: &[TreeNode], node: usize, self_idx: usize, p: [f32; 2], forces: &mut [[f32; 2]]) {
    let n = &tree[node];
    if n.mass == 0.0 {
        return;
    }
    let (dx, dy) = (p[0] - n.com[0], p[1] - n.com[1]);
    let d2 = (dx * dx + dy * dy).max(4.0);
    let dist = d2.sqrt();
    let is_self_leaf = n.point == self_idx as i32 && n.children.iter().all(|c| *c == -1);
    if is_self_leaf {
        return;
    }
    if n.children.iter().all(|c| *c == -1) || n.size / dist < THETA {
        let f = REPULSION * n.mass / d2 / dist;
        forces[self_idx][0] += dx * f;
        forces[self_idx][1] += dy * f;
        return;
    }
    for child in n.children {
        if child >= 0 {
            apply_repulsion(tree, child as usize, self_idx, p, forces);
        }
    }
}

// -- layout cache -----------------------------------------------------------

const CACHE_MAGIC: &[u8; 5] = b"LKGL1";

pub fn save_cache(dir: &Path, data: &GraphData, pos: &[[f32; 2]]) {
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(CACHE_MAGIC);
    let notes: Vec<usize> = (0..data.nodes.len()).filter(|&i| !data.nodes[i].is_tag).collect();
    buf.extend_from_slice(&(notes.len() as u32).to_le_bytes());
    for i in notes {
        buf.extend_from_slice(&data.nodes[i].rowid.to_le_bytes());
        buf.extend_from_slice(&pos[i][0].to_le_bytes());
        buf.extend_from_slice(&pos[i][1].to_le_bytes());
    }
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join("graph_layout.bin"), buf);
}

pub fn load_cache(dir: &Path) -> HashMap<i64, [f32; 2]> {
    let mut out = HashMap::new();
    let Ok(bytes) = std::fs::read(dir.join("graph_layout.bin")) else { return out };
    if bytes.len() < 9 || &bytes[0..5] != CACHE_MAGIC {
        return out;
    }
    let count = u32::from_le_bytes(bytes[5..9].try_into().unwrap()) as usize;
    let mut off = 9;
    for _ in 0..count {
        if off + 16 > bytes.len() {
            break;
        }
        let rowid = i64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        let x = f32::from_le_bytes(bytes[off + 8..off + 12].try_into().unwrap());
        let y = f32::from_le_bytes(bytes[off + 12..off + 16].try_into().unwrap());
        out.insert(rowid, [x, y]);
        off += 16;
    }
    out
}

// -- hit-test grid ------------------------------------------------------------

pub fn build_grid(pos: &[[f32; 2]], cell: f32) -> HashMap<(i32, i32), Vec<u32>> {
    let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for (i, p) in pos.iter().enumerate() {
        grid.entry(((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32))
            .or_default()
            .push(i as u32);
    }
    grid
}

/// Nearest node within `radius` world units of `world`, via the grid.
pub fn nearest(grid: &HashMap<(i32, i32), Vec<u32>>, pos: &[[f32; 2]], world: [f32; 2], radius: f32, cell: f32) -> Option<usize> {
    let (cx, cy) = ((world[0] / cell).floor() as i32, (world[1] / cell).floor() as i32);
    let mut best: Option<(usize, f32)> = None;
    for gx in (cx - 1)..=(cx + 1) {
        for gy in (cy - 1)..=(cy + 1) {
            if let Some(ids) = grid.get(&(gx, gy)) {
                for &id in ids {
                    let p = pos[id as usize];
                    let d = (p[0] - world[0]).powi(2) + (p[1] - world[1]).powi(2);
                    if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                        best = Some((id as usize, d));
                    }
                }
            }
        }
    }
    match best {
        Some((i, d)) if d <= radius * radius => Some(i),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("lazykasten-graph-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    fn seeded() -> (Store, PathBuf) {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        store.save("20260101000001", "Alpha\nlinks [[20260101000002]] and [[20260101000003]]\nTag: Rust").unwrap();
        store.save("20260101000002", "Beta\nlinks [[20260101000003]]\nTag: Rust").unwrap();
        store.save("20260101000003", "Gamma\nno links\nTag: Cooking").unwrap();
        store.save("20260101000004", "Orphan\nno links, unique tag\nTag: Solo").unwrap();
        (store, root)
    }

    #[test]
    fn test_graph_loader_modes() {
        let (store, root) = seeded();
        // Global without orphans/tags: only linked notes (Alpha, Beta, Gamma).
        let g = build(&store, GraphMode::Global, None, 2, false, false).unwrap();
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.edges.len(), 3);
        // With orphans and tags, the orphan and its tag nodes join.
        let g2 = build(&store, GraphMode::Global, None, 2, true, true).unwrap();
        assert_eq!(g2.nodes.len(), 4 + 3);
        assert!(g2.nodes.iter().any(|n| n.is_tag && n.id == "Solo"));
        // Local depth 1 around Alpha: Alpha, Beta, Gamma.
        let local = build(&store, GraphMode::Local, Some("20260101000001"), 1, false, false).unwrap();
        assert_eq!(local.nodes.len(), 3);
        // Depth 0: the center alone.
        let solo = build(&store, GraphMode::Local, Some("20260101000001"), 0, false, false).unwrap();
        assert_eq!(solo.nodes.len(), 1);
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_tag_over_cap_is_skipped() {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        for i in 0..5 {
            store.save(&format!("2026010100000{i}"), &format!("N{i}\nbody\nTag: Huge")).unwrap();
        }
        let g = build_with_cap(&store, GraphMode::Global, None, 2, true, true, 3).unwrap();
        assert!(!g.nodes.iter().any(|n| n.is_tag), "oversized tag must be skipped: {:?}", g.nodes.iter().map(|n| &n.id).collect::<Vec<_>>());
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn lcg(seed: &mut u64) -> f32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*seed >> 33) as f32) / (u32::MAX as f32)
    }

    #[test]
    fn test_barnes_hut_approximates_brute_force() {
        let mut seed = 42u64;
        let pos: Vec<[f32; 2]> = (0..80)
            .map(|_| [(lcg(&mut seed) - 0.5) * 400.0, (lcg(&mut seed) - 0.5) * 400.0])
            .collect();
        let data = GraphData { nodes: vec![], edges: vec![], edge_is_tag: vec![], degree: vec![], by_rowid: HashMap::new(), by_id: HashMap::new() };
        let bh = compute_forces(&data, &pos, true);
        let brute = compute_forces(&data, &pos, false);
        let mut diff = 0.0f32;
        let mut total = 0.0f32;
        for i in 0..pos.len() {
            diff += (bh[i][0] - brute[i][0]).abs() + (bh[i][1] - brute[i][1]).abs();
            total += brute[i][0].abs() + brute[i][1].abs();
        }
        assert!(diff / total.max(1.0) < 0.35, "BH vs brute relative error {}", diff / total.max(1.0));
    }

    #[test]
    fn test_layout_is_deterministic() {
        let (store, root) = seeded();
        let data = build(&store, GraphMode::Global, None, 2, true, true).unwrap();
        let run = || {
            let mut layout = Layout::new(&data, &HashMap::new());
            for _ in 0..40 {
                layout.step(&data, &HashSet::new());
            }
            layout.pos.clone()
        };
        assert_eq!(run(), run());
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_grid_nearest_matches_brute_force() {
        let mut seed = 7u64;
        let pos: Vec<[f32; 2]> = (0..120)
            .map(|_| [(lcg(&mut seed) - 0.5) * 500.0, (lcg(&mut seed) - 0.5) * 500.0])
            .collect();
        let grid = build_grid(&pos, 20.0);
        for _ in 0..30 {
            let q = [(lcg(&mut seed) - 0.5) * 500.0, (lcg(&mut seed) - 0.5) * 500.0];
            let via_grid = nearest(&grid, &pos, q, 15.0, 20.0);
            let brute = (0..pos.len())
                .filter(|&i| (pos[i][0] - q[0]).powi(2) + (pos[i][1] - q[1]).powi(2) <= 225.0)
                .min_by(|&a, &b| {
                    let da = (pos[a][0] - q[0]).powi(2) + (pos[a][1] - q[1]).powi(2);
                    let db = (pos[b][0] - q[0]).powi(2) + (pos[b][1] - q[1]).powi(2);
                    da.total_cmp(&db)
                });
            assert_eq!(via_grid, brute, "query {q:?}");
        }
    }

    #[test]
    fn test_cache_round_trip() {
        let (store, root) = seeded();
        let data = build(&store, GraphMode::Global, None, 2, false, false).unwrap();
        let layout = Layout::new(&data, &HashMap::new());
        let dir = root.join(".lazykasten");
        save_cache(&dir, &data, &layout.pos);
        let loaded = load_cache(&dir);
        for node in &data.nodes {
            let idx = data.by_rowid[&node.rowid];
            assert_eq!(loaded[&node.rowid], layout.pos[idx]);
        }
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }
}

