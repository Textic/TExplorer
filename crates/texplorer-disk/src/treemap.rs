use crate::tree::{DiskTree, NodeId};

/// Compact legend categories displayed alongside the Treemap header bar.
pub const TREEMAP_LEGEND_ITEMS: [(&str, u32); 7] = [
    ("Video", 0x4f46e5),
    ("Images", 0x0d9488),
    ("Archives", 0xd97706),
    ("Binaries", 0x0284c7),
    ("Code", 0x059669),
    ("Docs", 0x7c3aed),
    ("Disk/Sys", 0xdc2626),
];

/// A single positioned rectangle in the squarified Treemap.
#[derive(Debug, Clone, Copy)]
pub struct TreemapRect {
    pub node_id: NodeId,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color_hex: u32,
    pub depth: u8,
    pub is_dir: bool,
}

/// Inline header bar reserved at the top of prominent depth-1 folder containers.
#[derive(Debug, Clone, Copy)]
pub struct TreemapHeader {
    pub node_id: NodeId,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Precomputed Treemap layout ready for single-pass GPU canvas painting and hit-testing.
#[derive(Debug, Clone, Default)]
pub struct TreemapLayout {
    pub root_id: NodeId,
    pub width: f32,
    pub height: f32,
    /// Folder container plates (depth 1 and depth 2) rendered underneath leaves.
    pub frames: Vec<TreemapRect>,
    /// Inline folder headers painted at the top of large depth-1 containers.
    pub headers: Vec<TreemapHeader>,
    /// Leaf tiles (files or collapsed small directories) rendered on top.
    pub leaves: Vec<TreemapRect>,
}

const MAX_RENDER_DEPTH: u8 = 4;
const MAX_TILES: usize = 6_000;
const MIN_TILE_SIDE: f32 = 3.0;
const MIN_SPLIT_SIDE: f32 = 16.0;
const HEADER_H: f32 = 18.0;
const MIN_HEADER_W: f32 = 64.0;
const MIN_HEADER_CONTAINER_H: f32 = 42.0;

impl TreemapLayout {
    /// Computes a nested Squarified Treemap layout for `root_id` inside `width x height`.
    pub fn compute(
        tree: &DiskTree,
        root_id: NodeId,
        width: f32,
        height: f32,
        show_hidden: bool,
    ) -> Self {
        let mut layout = Self {
            root_id,
            width,
            height,
            frames: Vec::with_capacity(256),
            headers: Vec::with_capacity(64),
            leaves: Vec::with_capacity(1024),
        };
        if width < 4.0 || height < 4.0 {
            return layout;
        }
        let Some(root_node) = tree.get(root_id) else {
            return layout;
        };
        if root_node.removed || root_node.logical_size == 0 {
            return layout;
        }

        layout.layout_dir(tree, root_id, 0.0, 0.0, width, height, 0, show_hidden);
        layout
    }

    /// Hit-tests local `(x, y)` inside the canvas, returning the deepest tile or header node.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<NodeId> {
        for hdr in &self.headers {
            if x >= hdr.x && x <= hdr.x + hdr.w && y >= hdr.y && y <= hdr.y + hdr.h {
                return Some(hdr.node_id);
            }
        }
        for tile in self.leaves.iter().rev() {
            if x >= tile.x && x <= tile.x + tile.w && y >= tile.y && y <= tile.y + tile.h {
                return Some(tile.node_id);
            }
        }
        for frame in self.frames.iter().rev() {
            if x >= frame.x && x <= frame.x + frame.w && y >= frame.y && y <= frame.y + frame.h {
                return Some(frame.node_id);
            }
        }
        None
    }

    /// Finds the best bounding rectangle to highlight for `node_id` (falling back to nearest
    /// rendered ancestor if `node_id` is deeper than `MAX_RENDER_DEPTH`).
    pub fn highlight_rect_for_node(&self, tree: &DiskTree, node_id: NodeId) -> Option<TreemapRect> {
        let mut cursor = Some(node_id);
        let mut guard = 0u32;
        while let Some(cid) = cursor {
            guard += 1;
            if guard > 64 {
                break;
            }
            if let Some(r) = self.leaves.iter().find(|r| r.node_id == cid) {
                return Some(*r);
            }
            if let Some(r) = self.frames.iter().find(|r| r.node_id == cid) {
                return Some(*r);
            }
            if cid == self.root_id {
                break;
            }
            cursor = tree.get(cid).and_then(|n| n.parent);
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_dir(
        &mut self,
        tree: &DiskTree,
        dir_id: NodeId,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        depth: u8,
        show_hidden: bool,
    ) {
        if self.leaves.len() >= MAX_TILES || w < MIN_TILE_SIDE || h < MIN_TILE_SIDE {
            return;
        }

        let mut items: Vec<(NodeId, u64)> = tree
            .children(dir_id)
            .filter_map(|cid| {
                let n = tree.get(cid)?;
                if n.removed || n.logical_size == 0 {
                    return None;
                }
                if !show_hidden && n.is_hidden_or_system() {
                    return None;
                }
                Some((cid, n.logical_size))
            })
            .collect();

        if items.is_empty() {
            return;
        }

        items.sort_unstable_by_key(|a| std::cmp::Reverse(a.1));

        let total_size: u64 = items.iter().map(|(_, s)| *s).sum();
        if total_size == 0 {
            return;
        }

        let area = f64::from(w) * f64::from(h);
        let mut positioned: Vec<(NodeId, f32, f32, f32, f32)> = Vec::with_capacity(items.len());
        squarify_into(&items, total_size, area, x, y, w, h, &mut positioned);

        let child_depth = depth.saturating_add(1);
        for (cid, rx, ry, rw, rh) in positioned {
            if self.leaves.len() >= MAX_TILES {
                break;
            }
            if rw < MIN_TILE_SIDE || rh < MIN_TILE_SIDE {
                continue;
            }
            let Some(node) = tree.get(cid) else {
                continue;
            };

            if node.is_dir()
                && child_depth < MAX_RENDER_DEPTH
                && rw >= MIN_SPLIT_SIDE
                && rh >= MIN_SPLIT_SIDE
                && tree.children(cid).any(|gc| {
                    let g = &tree.nodes[gc as usize];
                    !g.removed && g.logical_size > 0 && (show_hidden || !g.is_hidden_or_system())
                })
            {
                let pad = if child_depth == 1 { 2.0 } else { 1.0 };
                let has_header =
                    child_depth == 1 && rw >= MIN_HEADER_W && rh >= MIN_HEADER_CONTAINER_H;

                if child_depth <= 2 {
                    self.frames.push(TreemapRect {
                        node_id: cid,
                        x: rx,
                        y: ry,
                        w: rw,
                        h: rh,
                        color_hex: if child_depth == 1 { 0x131317 } else { 0x19191f },
                        depth: child_depth,
                        is_dir: true,
                    });
                }

                let inner_x = rx + pad;
                let inner_w = (rw - pad * 2.0).max(0.0);
                let (inner_y, inner_h) = if has_header {
                    self.headers.push(TreemapHeader {
                        node_id: cid,
                        x: rx + 1.0,
                        y: ry + 1.0,
                        w: (rw - 2.0).max(0.0),
                        h: HEADER_H,
                    });
                    (ry + HEADER_H + 1.0, (rh - HEADER_H - pad - 1.0).max(0.0))
                } else {
                    (ry + pad, (rh - pad * 2.0).max(0.0))
                };

                let before_leaves = self.leaves.len();
                self.layout_dir(
                    tree,
                    cid,
                    inner_x,
                    inner_y,
                    inner_w,
                    inner_h,
                    child_depth,
                    show_hidden,
                );
                if self.leaves.len() == before_leaves {
                    self.leaves.push(TreemapRect {
                        node_id: cid,
                        x: inner_x,
                        y: inner_y,
                        w: inner_w.max(1.0),
                        h: inner_h.max(1.0),
                        color_hex: color_for_node(tree, cid),
                        depth: child_depth,
                        is_dir: true,
                    });
                }
            } else {
                self.leaves.push(TreemapRect {
                    node_id: cid,
                    x: rx,
                    y: ry,
                    w: rw,
                    h: rh,
                    color_hex: color_for_node(tree, cid),
                    depth: child_depth,
                    is_dir: node.is_dir(),
                });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn squarify_into(
    items: &[(NodeId, u64)],
    total_size: u64,
    total_area: f64,
    mut x: f32,
    mut y: f32,
    mut w: f32,
    mut h: f32,
    out: &mut Vec<(NodeId, f32, f32, f32, f32)>,
) {
    let scale = total_area / (total_size as f64);
    let mut idx = 0usize;
    let mut row: Vec<(NodeId, f64)> = Vec::with_capacity(32);

    while idx < items.len() && w > 0.5 && h > 0.5 {
        row.clear();
        let short_side = f64::from(w.min(h));
        let mut row_area = 0.0f64;

        while idx < items.len() {
            let (nid, size) = items[idx];
            let item_area = (size as f64) * scale;
            if item_area <= 0.0 {
                idx += 1;
                continue;
            }
            if row.is_empty() {
                row.push((nid, item_area));
                row_area = item_area;
                idx += 1;
                continue;
            }

            let cur_worst = worst_aspect(&row, row_area, short_side);
            let next_area = row_area + item_area;
            let next_worst = worst_aspect_with(&row, item_area, next_area, short_side);
            if next_worst <= cur_worst {
                row.push((nid, item_area));
                row_area = next_area;
                idx += 1;
            } else {
                break;
            }
        }

        if row.is_empty() || row_area <= 0.0 {
            break;
        }

        if w >= h {
            let strip_w = ((row_area / f64::from(h)) as f32).min(w);
            let mut cy = y;
            let denom = row_area.max(1e-9);
            for (i, &(nid, item_area)) in row.iter().enumerate() {
                let ih = if i + 1 == row.len() {
                    (y + h - cy).max(0.0)
                } else {
                    ((item_area / denom) * f64::from(h)) as f32
                };
                out.push((nid, x, cy, strip_w, ih));
                cy += ih;
            }
            x += strip_w;
            w = (w - strip_w).max(0.0);
        } else {
            let strip_h = ((row_area / f64::from(w)) as f32).min(h);
            let mut cx = x;
            let denom = row_area.max(1e-9);
            for (i, &(nid, item_area)) in row.iter().enumerate() {
                let iw = if i + 1 == row.len() {
                    (x + w - cx).max(0.0)
                } else {
                    ((item_area / denom) * f64::from(w)) as f32
                };
                out.push((nid, cx, y, iw, strip_h));
                cx += iw;
            }
            y += strip_h;
            h = (h - strip_h).max(0.0);
        }
    }
}

#[inline]
fn worst_aspect(row: &[(NodeId, f64)], row_area: f64, side: f64) -> f64 {
    let mut min_a = f64::INFINITY;
    let mut max_a = 0.0f64;
    for &(_, a) in row {
        if a < min_a {
            min_a = a;
        }
        if a > max_a {
            max_a = a;
        }
    }
    aspect_ratio(min_a, max_a, row_area, side)
}

#[inline]
fn worst_aspect_with(row: &[(NodeId, f64)], extra: f64, row_area: f64, side: f64) -> f64 {
    let mut min_a = extra;
    let mut max_a = extra;
    for &(_, a) in row {
        if a < min_a {
            min_a = a;
        }
        if a > max_a {
            max_a = a;
        }
    }
    aspect_ratio(min_a, max_a, row_area, side)
}

#[inline]
fn aspect_ratio(min_a: f64, max_a: f64, sum_a: f64, side: f64) -> f64 {
    if min_a <= 0.0 || sum_a <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let s2 = side * side;
    let r2 = sum_a * sum_a;
    let a = (s2 * max_a) / r2;
    let b = r2 / (s2 * min_a);
    a.max(b)
}

/// Maps a node (extension or folder) to a curated high-contrast flat color hex.
pub fn color_for_node(tree: &DiskTree, id: NodeId) -> u32 {
    let Some(node) = tree.get(id) else {
        return 0x52525b;
    };
    if node.is_dir() {
        let hash = fnv1a_hash(node.name.as_bytes());
        const DIR_PALETTE: [u32; 6] = [
            0x2d4a66, 0x3b3f6b, 0x26544d, 0x5c4632, 0x4b3863, 0x3f4c59,
        ];
        return DIR_PALETTE[(hash as usize) % DIR_PALETTE.len()];
    }
    let ext = tree.ext_str(id);
    color_for_extension(ext)
}

pub fn color_for_extension(ext: &str) -> u32 {
    match ext {
        // Video
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "wmv" | "flv" | "m4v" | "m2ts" => 0x4f46e5,
        // Audio
        "mp3" | "wav" | "flac" | "ogg" | "aac" | "m4a" | "opus" | "wma" => 0x2563eb,
        // Images & design
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "svg" | "ico" | "psd" | "tif"
        | "tiff" | "raw" | "heic" | "avif" => 0x0d9488,
        // Archives & packages
        "zip" | "7z" | "rar" | "tar" | "gz" | "xz" | "zst" | "bz2" | "cab" | "msi" | "msix"
        | "nupkg" | "whl" | "apk" => 0xd97706,
        // Disk images, VM & system
        "iso" | "img" | "vhd" | "vhdx" | "vmdk" | "vdi" | "qcow2" | "wim" | "esd" | "sys"
        | "dmp" | "hiberfil" | "pagefile" => 0xdc2626,
        // Executables & libraries
        "exe" | "dll" | "so" | "dylib" | "pdb" | "lib" | "rlib" | "obj" | "o" | "a" | "wasm"
        | "bin" | "pak" | "uasset" | "cas" => 0x0284c7,
        // Source code & config
        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "c" | "cpp" | "cc" | "h" | "hpp"
        | "cs" | "java" | "kt" | "swift" | "rb" | "php" | "lua" | "sh" | "ps1" | "bat" | "json"
        | "toml" | "yaml" | "yml" | "xml" | "html" | "css" | "scss" | "sql" => 0x059669,
        // Documents & databases
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" | "rtf" | "csv"
        | "db" | "sqlite" | "sqlite3" | "parquet" | "log" => 0x7c3aed,
        // Fallback neutral slate
        _ => 0x52525b,
    }
}

#[inline]
fn fnv1a_hash(bytes: &[u8]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for &b in bytes {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::tree::{NodeFlags, ScanEngineKind};

    #[test]
    fn squarified_layout_stays_within_bounds_and_hit_tests() {
        let mut tree = DiskTree::new_root(
            "X:\\",
            PathBuf::from("X:\\"),
            false,
            ScanEngineKind::ParallelWalk,
        );
        let dir = tree.add_node(tree.root, "Videos", 0, 0, 0, NodeFlags::DIR);
        let big = tree.add_node(dir, "capture.mp4", 600, 600, 0, NodeFlags::empty());
        let med = tree.add_node(tree.root, "archive.7z", 300, 300, 0, NodeFlags::empty());
        let sml = tree.add_node(tree.root, "main.rs", 100, 100, 0, NodeFlags::empty());
        tree.finalize_rollups();

        let layout = TreemapLayout::compute(&tree, tree.root, 800.0, 400.0, true);
        assert_eq!(layout.leaves.len(), 3);
        for rect in &layout.leaves {
            assert!(rect.x >= -0.5 && rect.y >= -0.5);
            assert!(rect.x + rect.w <= 800.5 && rect.y + rect.h <= 400.5);
        }

        let big_rect = layout.leaves.iter().find(|r| r.node_id == big).unwrap();
        assert_eq!(
            layout.hit_test(big_rect.x + big_rect.w * 0.5, big_rect.y + big_rect.h * 0.5),
            Some(big)
        );
        assert!(layout.highlight_rect_for_node(&tree, dir).is_some());
        let _ = (med, sml);
    }
}
