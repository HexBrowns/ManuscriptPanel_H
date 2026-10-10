//! 保存済みの `.aup2` から、テキストオブジェクトの本文を読む（表示中でないシーンの照合用。見るだけ）。
//! 読み方は TextReplace_H の `aup2.rs` を写して、本文だけに絞ったもの（共有クレートにはしない）。

use std::collections::HashMap;

/// テキストオブジェクト 1 つ
#[derive(Debug, Clone, PartialEq)]
pub struct SavedText {
    pub scene: i32,
    pub layer: usize,
    pub start: usize,
    pub end: usize,
    /// 保存形式のまま（`\n` は 2 文字）
    pub raw: String,
}

#[derive(Debug, Default)]
pub struct Saved {
    pub scenes: HashMap<i32, String>,
    pub texts: Vec<SavedText>,
}

pub fn parse(text: &str) -> Saved {
    enum Section {
        Other,
        Scene(i32),
        Object(usize),
        Effect(usize),
    }
    struct Obj {
        scene: i32,
        layer: usize,
        start: usize,
        end: usize,
        effect: String,
        raw: Option<String>,
    }
    let mut saved = Saved::default();
    let mut objects: Vec<Obj> = Vec::new();
    let mut index_of: HashMap<usize, usize> = HashMap::new();
    let mut section = Section::Other;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = if let Some(n) = name.strip_prefix("scene.") {
                n.parse().map(Section::Scene).unwrap_or(Section::Other)
            } else if let Some((obj, eff)) = name.split_once('.') {
                match (obj.parse::<usize>(), eff.parse::<usize>()) {
                    (Ok(o), Ok(_)) if index_of.contains_key(&o) => {
                        let idx = index_of[&o];
                        objects[idx].effect.clear();
                        Section::Effect(idx)
                    }
                    _ => Section::Other,
                }
            } else if let Ok(o) = name.parse::<usize>() {
                index_of.insert(o, objects.len());
                objects.push(Obj { scene: 0, layer: 0, start: 0, end: 0, effect: String::new(), raw: None });
                Section::Object(objects.len() - 1)
            } else {
                Section::Other
            };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        match section {
            Section::Scene(id) if key == "name" => {
                saved.scenes.insert(id, value.to_string());
            }
            Section::Object(idx) => {
                let o = &mut objects[idx];
                match key {
                    "layer" => o.layer = value.parse().unwrap_or(0),
                    "scene" => o.scene = value.parse().unwrap_or(0),
                    "frame" => {
                        let f: Vec<usize> = value.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                        o.start = f.first().copied().unwrap_or(0);
                        o.end = f.last().copied().unwrap_or(0);
                    }
                    _ => {}
                }
            }
            Section::Effect(idx) => {
                let o = &mut objects[idx];
                if key == "effect.name" {
                    o.effect = value.to_string();
                } else if o.effect == "テキスト" && key == "テキスト" && o.raw.is_none() {
                    o.raw = Some(value.to_string());
                }
            }
            _ => {}
        }
    }
    for o in objects {
        if let Some(raw) = o.raw {
            saved.texts.push(SavedText { scene: o.scene, layer: o.layer, start: o.start, end: o.end, raw });
        }
    }
    saved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_objects() {
        let t = "[project]\nversion=2011000\n[scene.0]\nname=Root\n[scene.1]\nname=Scene2\n\
                 [0]\nlayer=3\nframe=120,239\n[0.0]\neffect.name=テキスト\nサイズ=80.00\nテキスト=言葉が\\n流れる\n[0.1]\neffect.name=標準描画\nX=0.00\n\
                 [1]\nlayer=1\nscene=1\nframe=0,59\n[1.0]\neffect.name=図形\n[1.1]\neffect.name=標準描画\n\
                 [2]\nlayer=5\nscene=1\nframe=60,89\n[2.0]\neffect.name=テキスト\nテキスト=二つ目<//原稿:P:2//>\n";
        let s = parse(t);
        assert_eq!(s.scenes.get(&1).map(String::as_str), Some("Scene2"));
        assert_eq!(s.texts.len(), 2);
        assert_eq!(s.texts[0], SavedText { scene: 0, layer: 3, start: 120, end: 239, raw: "言葉が\\n流れる".into() });
        assert_eq!((s.texts[1].scene, s.texts[1].layer, s.texts[1].raw.as_str()), (1, 5, "二つ目<//原稿:P:2//>"));
    }
}
