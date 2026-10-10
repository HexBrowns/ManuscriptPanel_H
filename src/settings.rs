//! 設定（`Plugin/ManuscriptPanel_H/設定.json`）。プロジェクトごとの原稿の場所は、プロジェクトの中に持つ（`lib.rs`）

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::manuscript::{Options, Unit, DEFAULT_CUT_PATTERN};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LengthMode {
    Frames,
    ByChars,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClickMode {
    Single,
    Double,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 型（`Alias/` からの相対の名前、拡張子なし）。空なら本体の初期値のテキスト
    pub type_name: String,
    /// 置くレイヤー（1 始まり）。None なら選んでいるレイヤー
    pub fixed_layer: Option<usize>,
    pub length_mode: LengthMode,
    pub frames: usize,
    pub sec_per_char: f64,
    pub margin_sec: f64,
    pub min_frames: usize,
    pub continuous: bool,
    pub click: ClickMode,
    /// 本文の末尾に印（`<//原稿:…//>`）を付ける
    pub add_mark: bool,
    pub unit: Unit,
    pub cut_pattern: String,
    pub memo_prefix: String,
    pub show_non_cut: bool,
    pub show_memos: bool,
    /// 置いた行を一覧から隠す
    pub hide_placed: bool,
    /// プロジェクトに覚えが無いときに開く、最後に開いた原稿
    pub last_file: Option<PathBuf>,
    /// 見出しの型（`#` の数 - 1 番目）。None ならその見出しは置かない。Some("") は本体の初期値（第 2 段階）
    pub heading_types: Vec<Option<String>>,
    /// まとめて置くときの、テロップの間の空き（フレーム）
    pub gap: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            type_name: String::new(),
            fixed_layer: None,
            length_mode: LengthMode::Frames,
            frames: 90,
            // 読む速さの上限 1 秒 4 文字（Netflix 日本語字幕ガイド）と、最短の表示 5/6 秒 = 25 フレーム（スキル au2-video-knowledge の 2.5）
            sec_per_char: 0.25,
            margin_sec: 0.5,
            min_frames: 25,
            continuous: true,
            click: ClickMode::Single,
            add_mark: true,
            unit: Unit::Line,
            cut_pattern: DEFAULT_CUT_PATTERN.into(),
            memo_prefix: "▸".into(),
            show_non_cut: false,
            show_memos: true,
            hide_placed: false,
            last_file: None,
            heading_types: vec![None; 6],
            gap: 0,
        }
    }
}

impl Settings {
    /// その深さの見出しの型（置かないなら None）
    pub fn heading_type(&self, level: usize) -> Option<&String> {
        level.checked_sub(1).and_then(|k| self.heading_types.get(k)).and_then(|t| t.as_ref())
    }

    pub fn options(&self) -> Options {
        Options {
            unit: self.unit,
            cut_pattern: self.cut_pattern.clone(),
            memo_prefix: self.memo_prefix.clone(),
            show_non_cut: self.show_non_cut,
        }
    }
}

pub fn path() -> PathBuf {
    aviutl2::config::app_data_path().join("Plugin").join("ManuscriptPanel_H").join("設定.json")
}

pub fn load() -> Settings {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

pub fn save(s: &Settings) -> Result<(), String> {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| format!("設定を保存できませんでした: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults() {
        let s: Settings = serde_json::from_str(r#"{"frames": 60}"#).unwrap();
        assert_eq!(s.frames, 60);
        assert_eq!(s.cut_pattern, DEFAULT_CUT_PATTERN);
        assert!(s.add_mark);
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
