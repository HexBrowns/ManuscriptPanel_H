//! 開いている原稿の監視。Obsidian で保存したら一覧を読み直す（読み直すだけで、本体へは書かない）。
//!
//! - ファイルの入ったフォルダを（下の階層は見ずに）見て、そのファイル名の変化だけを拾う。保存のしかたが
//!   「一時ファイルに書いてから置き換える」でも拾える
//! - 自分ではスレッドを起こさない。`notify` の中のスレッドが印（最後に変わった時刻）を付け、UI が毎フレーム見て、
//!   変化が `QUIET` だけ止んだら読み直す
//! - `notify` の中のスレッドはこちらから待てないので、監視を始めるときに DLL を pin する（ルール au2-rs-plugin
//!   「待てないスレッドが残るなら DLL を pin する」）。監視はプラグインの `Drop` で外す（`stop_all`）

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aviutl2_eframe::egui;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;

pub const QUIET: Duration = Duration::from_millis(400);

static CURRENT: Mutex<Option<RecommendedWatcher>> = Mutex::new(None);

/// 監視の印（最後に変わった時刻）
#[derive(Clone, Default)]
pub struct Dirty(Arc<Mutex<Option<Instant>>>);

impl Dirty {
    /// 読み直す時か。変化が止んでいれば印を下ろして true。まだなら、あと何秒で見直すかを返す
    pub fn poll(&self, now: Instant) -> Result<bool, Duration> {
        let mut d = self.0.lock();
        match *d {
            None => Ok(false),
            Some(t) if now.duration_since(t) >= QUIET => {
                *d = None;
                Ok(true)
            }
            Some(t) => Err(QUIET - now.duration_since(t)),
        }
    }

    fn mark(&self) {
        *self.0.lock() = Some(Instant::now());
    }
}

/// `file` の監視を始める（前の監視は外す）
pub fn watch(file: &Path, dirty: Dirty, ctx: egui::Context) -> Result<(), String> {
    crate::pin_module();
    let dir = file.parent().ok_or("フォルダが分かりません")?.to_path_buf();
    let name = file.file_name().ok_or("ファイル名が分かりません")?.to_string_lossy().to_lowercase();
    let handler = move |res: notify::Result<notify::Event>| {
        let hit = match res {
            // 取りこぼし（バッファあふれなど）は、分からないので読み直す
            Err(_) => true,
            Ok(ev) => {
                !matches!(ev.kind, EventKind::Access(_))
                    && (ev.need_rescan() || ev.paths.iter().any(|p| same_name(p, &name)))
            }
        };
        if hit {
            dirty.mark();
            ctx.request_repaint_after(QUIET);
        }
    };
    let mut w = notify::recommended_watcher(handler).map_err(|e| format!("監視を始められませんでした: {e}"))?;
    w.watch(&dir, RecursiveMode::NonRecursive).map_err(|e| format!("{} を監視できません: {e}", dir.display()))?;
    *CURRENT.lock() = Some(w);
    Ok(())
}

fn same_name(p: &Path, lower_name: &str) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase() == lower_name)
}

/// 監視を外す（プラグインの Drop から）
pub fn stop_all() {
    CURRENT.lock().take();
}

/// 原稿を読む（UTF-8。BOM は `manuscript::parse` が外す。UTF-8 でない並びは置き換えて読む）
pub fn read_text(path: &PathBuf) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{} を読めませんでした: {e}", path.display()))?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_quiet() {
        let d = Dirty::default();
        let t0 = Instant::now();
        assert_eq!(d.poll(t0), Ok(false));
        *d.0.lock() = Some(t0);
        assert!(d.poll(t0 + Duration::from_millis(100)).is_err());
        assert_eq!(d.poll(t0 + QUIET), Ok(true));
        // 読んだら下りる
        assert_eq!(d.poll(t0 + QUIET * 2), Ok(false));
    }

    #[test]
    fn detects_save_in_real_folder() {
        let dir = std::env::temp_dir().join(format!("manuscript_panel_h_watch_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("原稿.md");
        std::fs::write(&file, "【1】あ\n").unwrap();
        let other = dir.join("別.md");
        let d = Dirty::default();
        watch(&file, d.clone(), egui::Context::default()).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        // ほかのファイルの変化は拾わない
        std::fs::write(&other, "x").unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert!(d.0.lock().is_none(), "ほかのファイルで印が付いた");
        std::fs::write(&file, "【1】い\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while d.0.lock().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        stop_all();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(d.0.lock().is_some(), "保存を拾えなかった");
    }
}
