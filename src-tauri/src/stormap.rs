//! What is actually taking the space on a volume.
//!
//! The map is the whole volume, not a list of folders that happens to fit: the biggest
//! directories the walk could read, the free space beside them, and one region for every used
//! byte none of them covers. A directory the walker could not open is not 0 bytes and is not
//! free — it comes out of that unknown region, because a map that adds up to "free" when it
//! actually failed to open 400 GB is worse than no map.
//!
//! The part worth testing is [`layout`]: it takes what the walk measured and what the volume
//! itself reports and produces the boxes to draw, so its honesty can be checked without a real
//! disk. The walking below is deliberately boring — `read_dir` over a real tree, never following
//! a reparse point.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::ipc::AppState;
use crate::models::{StorageMap, StorageMapBox};

/// One thing the walker found: a directory with a measured size, or one it could not open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub bytes: u64,
    /// The directory could not be opened, so `bytes` is 0 and its real size is unknown. It is
    /// not measured and not a box; its bytes come out of the unknown region.
    pub could_not_read: bool,
}

/// A box to draw: its size and its share of the whole volume, whether it stands for space that
/// could not be read, and whether it is the free region.
#[derive(Clone, Debug, PartialEq)]
pub struct MapBox {
    pub path: String,
    pub bytes: u64,
    pub share: f64,
    pub unknown: bool,
    pub free: bool,
}

/// How many of the biggest entries get their own box; the rest fold into the unknown region, so
/// a busy drive never turns into a wall of slivers.
pub const MAX_BOXES: usize = 40;

/// The smallest directory worth opening into boxes of its own, as one part of the volume used.
/// A directory smaller than this is shown whole rather than broken into children nobody can read.
const MIN_BOX_SHARE: u64 = 400;
/// Below this no directory is opened however small the volume is.
const MIN_BOX_FLOOR: u64 = 16 * 1024 * 1024;
/// A tree deeper than this is treated as unreadable: real nesting never goes this far, and a
/// walk that recurses to the limit of the stack is a crash waiting on one pathological folder.
const MAX_DEPTH: usize = 64;
/// Progress is emitted at most this often while the walk runs, whatever the file count.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// The boxes to draw: the `top` biggest measured entries, then the part of used space none of
/// them covers (folders too small to show plus whatever could not be read), then the free space.
///
/// Shares are over `total`, so they add to 1 when `used + free == total`, which is how the
/// volume itself reports them. An entry that could not be read is not measured, so it never
/// becomes a box; an entry whose `bytes` is 0 is never a box either, because a zero-share box is
/// a hole nobody measured. Ties in size are broken by path, so the map does not reshuffle while
/// it is being looked at.
pub fn layout(entries: &[Entry], total: u64, used: u64, free: u64, top: usize) -> Vec<MapBox> {
    let mut boxes = Vec::new();
    if total == 0 {
        return boxes;
    }
    let mut measured: Vec<&Entry> = entries
        .iter()
        .filter(|e| !e.could_not_read && e.bytes > 0)
        .collect();
    measured.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));

    let mut shown_bytes = 0u64;
    for entry in measured.iter().take(top) {
        shown_bytes = shown_bytes.saturating_add(entry.bytes);
        boxes.push(MapBox {
            path: entry.path.clone(),
            bytes: entry.bytes,
            share: entry.bytes as f64 / total as f64,
            unknown: false,
            free: false,
        });
    }

    let unknown_bytes = used.saturating_sub(shown_bytes);
    if unknown_bytes > 0 {
        boxes.push(MapBox {
            path: String::new(),
            bytes: unknown_bytes,
            share: unknown_bytes as f64 / total as f64,
            unknown: true,
            free: false,
        });
    }
    if free > 0 {
        boxes.push(MapBox {
            path: String::new(),
            bytes: free,
            share: free as f64 / total as f64,
            unknown: false,
            free: true,
        });
    }
    boxes
}

/// What one walk of a volume produced.
pub struct Walk {
    pub entries: Vec<Entry>,
    /// Directories that could not be opened; their sizes are part of the unknown region.
    pub unreadable: u64,
    /// Files whose length was counted.
    pub files: u64,
    /// A newer scan (or a cancel) took over and this one stopped early.
    pub cancelled: bool,
}

/// The scan currently in charge. Starting one, or cancelling, bumps it; a walker that sees its
/// own generation gone knows a newer scan owns the answer and stops.
static SCAN: AtomicU64 = AtomicU64::new(0);

struct Ctx {
    unreadable: u64,
    files: u64,
    bytes: u64,
    generation: u64,
    cancelled: bool,
    last_emit: Instant,
    app: AppHandle,
    volume_id: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    volume_id: String,
    files: u64,
    bytes: u64,
    unreadable: u64,
}

impl Ctx {
    fn emit_progress(&mut self) {
        if self.last_emit.elapsed() < PROGRESS_EVERY {
            return;
        }
        self.last_emit = Instant::now();
        let progress = Progress {
            volume_id: self.volume_id.clone(),
            files: self.files,
            bytes: self.bytes,
            unreadable: self.unreadable,
        };
        let _ = self.app.emit("storage-map-progress", &progress);
    }
}

/// Whether a directory is a junction, a symlink or a mount point. Following one would count its
/// target twice — or in a circle, as `C:\Users\All Users` does — so reparse points are counted
/// for their own record and never descended.
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_REPARSE_POINT, from winnt.h.
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// The total size of `dir`, and how to draw it: a box for each child directory big enough to be
/// worth its own box (its children in turn), plus one box for `dir`'s own residual — its files,
/// its reparse points and its too-small directories. The two together partition `dir` exactly, so
/// nothing is counted twice and nothing is left out.
fn walk_dir(dir: &Path, min_bytes: u64, depth: usize, ctx: &mut Ctx) -> (u64, Vec<Entry>) {
    if ctx.cancelled || SCAN.load(Ordering::SeqCst) != ctx.generation {
        ctx.cancelled = true;
        return (0, Vec::new());
    }

    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(_) => {
            ctx.unreadable += 1;
            return (
                0,
                vec![Entry { path: dir.display().to_string(), bytes: 0, could_not_read: true }],
            );
        }
    };

    let mut total: u64 = 0;
    let mut boxes: Vec<Entry> = Vec::new();
    let mut residual: u64 = 0;

    for child in read {
        let child = match child {
            Ok(child) => child,
            Err(_) => {
                ctx.unreadable += 1;
                continue;
            }
        };
        let path = child.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => {
                ctx.unreadable += 1;
                continue;
            }
        };
        if is_reparse_point(&meta) {
            let len = meta.len();
            total += len;
            residual += len;
            ctx.files += 1;
            ctx.bytes = ctx.bytes.saturating_add(len);
            ctx.emit_progress();
            continue;
        }
        if meta.is_dir() {
            if depth + 1 > MAX_DEPTH {
                ctx.unreadable += 1;
                continue;
            }
            let (size, child_boxes) = walk_dir(&path, min_bytes, depth + 1, ctx);
            if ctx.cancelled {
                return (0, Vec::new());
            }
            total += size;
            if size >= min_bytes {
                boxes.extend(child_boxes);
            } else {
                residual += size;
            }
        } else {
            let len = meta.len();
            total += len;
            residual += len;
            ctx.files += 1;
            ctx.bytes = ctx.bytes.saturating_add(len);
        }
        ctx.emit_progress();
    }

    if residual > 0 {
        boxes.push(Entry { path: dir.display().to_string(), bytes: residual, could_not_read: false });
    }
    (total, boxes)
}

/// Walks `mount` and returns the boxes worth drawing, plus how many directories it could not open
/// and how many files it counted. `min_bytes` is the smallest directory worth opening into boxes.
pub fn walk_volume(
    mount: &str,
    min_bytes: u64,
    generation: u64,
    app: &AppHandle,
    volume_id: &str,
) -> Walk {
    let mut ctx = Ctx {
        unreadable: 0,
        files: 0,
        bytes: 0,
        generation,
        cancelled: false,
        last_emit: Instant::now(),
        app: app.clone(),
        volume_id: volume_id.to_string(),
    };
    let (_, entries) = walk_dir(&PathBuf::from(mount), min_bytes, 0, &mut ctx);
    Walk {
        entries,
        unreadable: ctx.unreadable,
        files: ctx.files,
        cancelled: ctx.cancelled,
    }
}

/// The last component of a box's path, which is what the map shows; the volume root itself has no
/// component, so it keeps its own name.
fn box_name(box_: &MapBox) -> String {
    Path::new(&box_.path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| box_.path.clone())
}

/// Maps one volume. The walk runs off the main thread (`spawn_blocking`) and can take minutes on
/// a full drive; a new scan, or a cancel, abandons the walk in flight. Nothing is walked until
/// this is called.
#[tauri::command]
pub async fn storage_map_scan(app: AppHandle, volume_id: String) -> Result<StorageMap, String> {
    let state = app.state::<AppState>();
    let (mount, total, used, free) = {
        let latest = state.latest.read().unwrap_or_else(|p| p.into_inner());
        let Some(snapshot) = latest.as_ref() else {
            return Err("no-reading".to_string());
        };
        let Some(volume) = snapshot.disk.volumes.iter().find(|v| v.id == volume_id) else {
            return Err("no-such-volume".to_string());
        };
        (
            volume.mount_point.clone(),
            volume.total_bytes,
            volume.used_bytes,
            volume.free_bytes,
        )
    };
    if total == 0 || mount.is_empty() {
        return Err("empty-volume".to_string());
    }

    let generation = SCAN.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
    let min_bytes = (used / MIN_BOX_SHARE).max(MIN_BOX_FLOOR);

    let walk_volume_id = volume_id.clone();
    let walk = tauri::async_runtime::spawn_blocking(move || {
        walk_volume(&mount, min_bytes, generation, &app, &walk_volume_id)
    })
    .await
    .map_err(|error| format!("walk did not finish: {error}"))?;

    let boxes: Vec<StorageMapBox> = layout(&walk.entries, total, used, free, MAX_BOXES)
        .into_iter()
        .map(|box_| StorageMapBox {
            name: box_name(&box_),
            path: box_.path,
            bytes: box_.bytes,
            share: box_.share,
            unknown: box_.unknown,
            free: box_.free,
        })
        .collect();
    let measured_bytes = walk.entries.iter().map(|e| e.bytes).sum();

    Ok(StorageMap {
        volume_id,
        total_bytes: total,
        used_bytes: used,
        free_bytes: free,
        measured_bytes,
        unreadable: walk.unreadable,
        scanned_files: walk.files,
        cancelled: walk.cancelled,
        boxes,
    })
}

/// Abandons the scan in flight, whatever window started it.
#[tauri::command]
pub fn storage_map_cancel() {
    SCAN.fetch_add(1, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, bytes: u64) -> Entry {
        Entry { path: path.to_string(), bytes, could_not_read: false }
    }

    fn shares(boxes: &[MapBox]) -> f64 {
        boxes.iter().map(|b| b.share).sum()
    }

    #[test]
    fn the_shares_add_to_one() {
        let entries = vec![entry("a", 300), entry("b", 200), entry("c", 100)];
        let boxes = layout(&entries, 1000, 600, 400, 40);
        assert!((shares(&boxes) - 1.0).abs() < 1e-9, "shares sum to {}", shares(&boxes));
    }

    #[test]
    fn an_unknown_amount_is_not_counted_as_free() {
        // One readable directory, the rest of used space unreadable: the unknown region holds
        // what was not measured, and the free region stays exactly the free space.
        let entries = vec![entry("a", 400)];
        let boxes = layout(&entries, 1000, 600, 400, 40);
        let free = boxes.iter().find(|b| b.free).expect("a free box");
        assert_eq!(free.bytes, 400);
        let unknown = boxes.iter().find(|b| b.unknown).expect("an unknown box");
        assert_eq!(unknown.bytes, 200);
    }

    #[test]
    fn a_drive_with_nothing_readable_is_one_unknown_box_not_an_empty_map() {
        let entries = vec![
            Entry { path: "x".into(), bytes: 0, could_not_read: true },
            Entry { path: "y".into(), bytes: 0, could_not_read: true },
        ];
        let boxes = layout(&entries, 500, 500, 0, 40);
        assert_eq!(boxes.len(), 1, "one box, the unknown remainder, rather than nothing");
        assert!(boxes[0].unknown);
        assert_eq!(boxes[0].bytes, 500);
    }

    #[test]
    fn a_zero_size_entry_never_becomes_a_zero_share_box() {
        let entries = vec![entry("empty", 0), entry("real", 250)];
        let boxes = layout(&entries, 1000, 500, 500, 40);
        assert!(boxes.iter().all(|b| b.bytes > 0 && b.share > 0.0));
        assert!(!boxes.iter().any(|b| b.path == "empty"));
    }

    #[test]
    fn ties_order_stably_by_path() {
        let entries = vec![entry("b", 100), entry("a", 100), entry("c", 100)];
        let boxes = layout(&entries, 1000, 300, 700, 40);
        let names: Vec<&str> = boxes.iter().filter(|b| !b.unknown && !b.free).map(|b| b.path.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn the_top_n_fold_the_rest_into_the_unknown_region() {
        let entries: Vec<Entry> = (0..50).map(|i| entry(&format!("d{i:02}"), 10)).collect();
        let boxes = layout(&entries, 1000, 500, 500, 3);
        let shown = boxes.iter().filter(|b| !b.unknown && !b.free).count();
        assert_eq!(shown, 3);
        let unknown = boxes.iter().find(|b| b.unknown).expect("an unknown box");
        assert_eq!(unknown.bytes, 470, "500 used minus the three boxes of 10");
    }
}
