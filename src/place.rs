//! 本体との読み書き（置く・照合）と、型（エイリアス）の扱い。
//!
//! - 書き込みは、一覧の行をクリックしたとき・ボタンを押したときの `call_edit_section` 1 回だけ（本体の Undo 1 回分）。
//!   ファイルの変化やフォーカスの変化では書かない（ルール au2-rs-plugin）
//! - 本文は型のエイリアスの `テキスト=` を書き換えてから作る（作った後に `set_effect_item` で書き換えない。1 回の作成で済み、
//!   書き換えの API が効くかどうかに頼らない）。作った後に本文を読み返して確かめる
//! - 読み取り（照合）は `call_read_section`。表示中でないシーンは保存済みの `.aup2` から読む（`aup2.rs`）

use aviutl2::generic::ObjectHandle;

use crate::EDIT_HANDLE;

pub const TEXT_EFFECT: &str = "テキスト";
pub const TEXT_ITEM: &str = "テキスト";

/// 型を選んでいないときに使う、本体の初期値のテキスト
pub const DEFAULT_ALIAS: &str = "[Object]\r\n[Object.0]\r\neffect.name=テキスト\r\nテキスト=\r\n[Object.1]\r\neffect.name=標準描画\r\n";

// ---- 保存形式 ----

/// 本体が返す形（`\n` = 改行、`\\` = `\`）を本文に戻す。ほかの `\` はそのまま（TextReplace_H の `text.rs` と同じ）
pub fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut it = raw.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.peek() {
                Some('n') => {
                    it.next();
                    out.push('\n');
                }
                Some('\\') => {
                    it.next();
                    out.push('\\');
                }
                _ => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 本文を本体へ書く形にする（`\` → `\\`、改行 → `\n`、CR は落とす）
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

// ---- 型（エイリアス） ----

/// 型として使えるか。1 個のオブジェクト（先頭が `[Object]`）で、最初の効果がテキストであること
pub fn check_alias(alias: &str) -> Result<(), String> {
    let alias = alias.trim_start_matches('\u{feff}');
    let mut lines = alias.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.trim().is_empty());
    if lines.next() != Some("[Object]") {
        return Err("1 個のオブジェクトのエイリアスではありません（複数のオブジェクトの型は使えません）".into());
    }
    let mut in_first = false;
    for line in lines {
        if line.starts_with('[') {
            if in_first {
                break;
            }
            in_first = line == "[Object.0]";
            continue;
        }
        if in_first {
            if let Some(name) = line.strip_prefix("effect.name=") {
                return if name == TEXT_EFFECT { Ok(()) } else { Err(format!("テキストの型ではありません（最初の効果が「{name}」）")) };
            }
        }
    }
    Err("テキストの型ではありません（最初の効果がテキストでない）".into())
}

/// 型のエイリアスの本文（`[Object.0]` の `テキスト=`）を `raw`（保存形式）に置き換える。無ければ `effect.name` の次に足す
pub fn set_alias_text(alias: &str, raw: &str) -> Result<String, String> {
    check_alias(alias)?;
    let alias = alias.trim_start_matches('\u{feff}');
    let eol = if alias.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out: Vec<String> = Vec::new();
    let mut in_first = false;
    let mut done = false;
    for line in alias.lines().map(|l| l.trim_end_matches('\r')) {
        if line.starts_with('[') {
            if in_first && !done {
                // テキストの項目が無かった。effect.name の次（区切りの前）に足す
                let at = out.iter().rposition(|l| l.starts_with("effect.name=")).map(|p| p + 1).unwrap_or(out.len());
                out.insert(at, format!("{TEXT_ITEM}={raw}"));
                done = true;
            }
            in_first = line == "[Object.0]";
            out.push(line.to_string());
            continue;
        }
        if in_first && !done && line.split_once('=').is_some_and(|(k, _)| k == TEXT_ITEM) {
            out.push(format!("{TEXT_ITEM}={raw}"));
            done = true;
            continue;
        }
        out.push(line.to_string());
    }
    if in_first && !done {
        out.push(format!("{TEXT_ITEM}={raw}"));
    }
    let mut s = out.join(eol);
    s.push_str(eol);
    Ok(s)
}

/// 型の一覧（`Alias/` とその一つ下の `.object` のうち、型として使えるもの）。名前は `Alias/` からの相対（拡張子なし）。
/// 使えなかった数も返す
pub fn list_types() -> (Vec<(String, std::path::PathBuf)>, usize) {
    let root = aviutl2::config::app_data_path().join("Alias");
    let mut out = Vec::new();
    let mut skipped = 0;
    let mut visit = |dir: &std::path::Path, prefix: &str, recurse: bool| -> Vec<(String, std::path::PathBuf)> {
        let mut subdirs = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else { return subdirs };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if recurse {
                    subdirs.push((e.file_name().to_string_lossy().into_owned(), p));
                }
                continue;
            }
            if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("object")) {
                continue;
            }
            let ok = std::fs::read(&p).ok().map(|b| String::from_utf8_lossy(&b).into_owned()).is_some_and(|t| check_alias(&t).is_ok());
            if ok {
                let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                out.push((format!("{prefix}{stem}"), p));
            } else {
                skipped += 1;
            }
        }
        subdirs
    };
    let subdirs = visit(&root, "", true);
    for (name, dir) in subdirs {
        visit(&dir, &format!("{name}/"), false);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    (out, skipped)
}

// ---- 置く ----

/// 長さの決め方
#[derive(Debug, Clone, Copy)]
pub enum Length {
    Frames(usize),
    /// 文字数 × 秒/字 + 余白（秒）。短すぎるときは最短のフレーム数
    ByChars { sec_per_char: f64, margin_sec: f64, min_frames: usize },
}

impl Length {
    pub fn frames(&self, text: &str, fps: f64) -> usize {
        match *self {
            Length::Frames(n) => n.max(1),
            Length::ByChars { sec_per_char, margin_sec, min_frames } => {
                let chars = crate::manuscript::visible_chars(text) as f64;
                (((chars * sec_per_char + margin_sec) * fps).ceil() as usize).max(min_frames).max(1)
            }
        }
    }
}

/// 「続けて置く」ための、直前に置いたときの記録
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Continue {
    pub scene: i32,
    pub layer: usize,
    /// 置いたときのカーソルのフレーム（ここから動いていなければ続きに置く）
    pub cursor: usize,
    /// 直前に置いたオブジェクトの終わりの次
    pub next: usize,
}

/// 置く 1 つ分
#[derive(Debug, Clone)]
pub struct Piece {
    /// 型のエイリアス（見出しと本文で型を変えられる）
    pub alias: String,
    /// 本文（印を含む。保存形式に直す前）
    pub body: String,
    /// 長さを文字数で決めるときに数える本文（印を含まない）
    pub text_for_length: String,
}

#[derive(Debug, Clone)]
pub struct PlaceReq {
    pub pieces: Vec<Piece>,
    pub length: Length,
    /// 置くレイヤー（0 始まり）。None なら選んでいるレイヤー
    pub layer: Option<usize>,
    /// 続けて置く（直前の記録）。None なら今のカーソル位置
    pub cont: Option<Continue>,
    /// 2 つ以上置くときの間隔（フレーム）
    pub gap: usize,
}

/// 置いたもの 1 つ
#[derive(Debug, Clone)]
pub struct PlacedObject {
    pub handle: ObjectHandle,
    pub layer: usize,
    pub start: usize,
    pub end: usize,
    /// 書いた本文（保存形式を戻したもの）
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct Placed {
    pub scene: i32,
    pub objects: Vec<PlacedObject>,
    pub cont: Continue,
    /// 本文が思ったとおりに入らなかったものなど
    pub warnings: Vec<String>,
}

fn ready() -> Result<(), String> {
    if EDIT_HANDLE.is_ready() {
        Ok(())
    } else {
        Err("本体の準備ができていません".into())
    }
}

/// 置き場所を決める（作り始める前に全部決める）
pub fn plan(start: usize, lengths: &[usize], gap: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(lengths.len());
    let mut f = start;
    for &len in lengths {
        out.push((f, f + len - 1));
        f += len + gap;
    }
    out
}

/// 置く（`call_edit_section` 1 回 = 本体の Undo 1 回分）。置く場所が 1 つでも埋まっていたら何も作らない
pub fn place(req: PlaceReq) -> Result<Placed, String> {
    ready()?;
    if req.pieces.is_empty() {
        return Err("置く行がありません".into());
    }
    // 本文を入れた型を先に作っておく（形の誤りはここで止める）
    let mut aliases = Vec::with_capacity(req.pieces.len());
    for p in &req.pieces {
        aliases.push((set_alias_text(&p.alias, &escape(&p.body))?, escape(&p.body)));
    }
    EDIT_HANDLE
        .call_edit_section(move |edit| -> Result<Placed, String> {
            let info = &edit.info;
            let scene = info.scene_id;
            let layer = req.layer.unwrap_or(info.layer);
            let cursor = info.frame;
            let fps = *info.fps.numer() as f64 / (*info.fps.denom()).max(1) as f64;
            let start = match req.cont {
                Some(c) if c.scene == scene && c.layer == layer && c.cursor == cursor => c.next,
                _ => cursor,
            };
            let lengths: Vec<usize> = req.pieces.iter().map(|p| req.length.frames(&p.text_for_length, fps)).collect();
            let places = plan(start, &lengths, req.gap);
            // 置き場所が空いているか（本体は重なると作れないか、長さ 0 だと位置をずらして置く）
            for &(s, e) in &places {
                let mut from = 0;
                for _ in 0..100_000 {
                    let Ok(Some(o)) = edit.find_object_after(layer, from) else { break };
                    let Ok(lf) = edit.get_object_layer_frame(o) else { break };
                    if lf.start <= e && lf.end >= s {
                        return Err(format!(
                            "{} レイヤー目の {s}〜{e} に別のオブジェクト（{}〜{}）があります。何も置きませんでした",
                            layer + 1,
                            lf.start,
                            lf.end
                        ));
                    }
                    if lf.start > e {
                        break;
                    }
                    from = lf.end + 1;
                }
            }
            let mut objects = Vec::new();
            let mut warnings = Vec::new();
            for (k, ((alias, raw), &(s, e))) in aliases.iter().zip(&places).enumerate() {
                let handle = edit.create_object_from_alias(alias, layer, s, e - s + 1).map_err(|err| {
                    let done = if objects.is_empty() { String::new() } else { format!("。それまでの {} 個は置きました", objects.len()) };
                    format!("{} レイヤー目・{s} に置けませんでした（{err}）{done}", layer + 1)
                })?;
                let (start, end) = edit.get_object_layer_frame(handle).map(|lf| (lf.start, lf.end)).unwrap_or((s, e));
                match edit.get_object_effect_item(handle, TEXT_EFFECT, 0, TEXT_ITEM) {
                    Ok(back) if &back == raw => {}
                    Ok(back) => {
                        tracing::warn!("[ManuscriptPanel_H] wrote {raw:?} read back {back:?}");
                        warnings.push(format!("{} 個目の本文が思ったとおりに入りませんでした", k + 1));
                    }
                    Err(err) => warnings.push(format!("{} 個目の本文を読み返せませんでした（{err}）", k + 1)),
                }
                if start != s || end != e {
                    warnings.push(format!("{} 個目は {s}〜{e} でなく {start}〜{end} に置かれました", k + 1));
                }
                objects.push(PlacedObject { handle, layer, start, end, body: unescape(raw) });
            }
            if let Some(last) = objects.last() {
                let _ = edit.set_focus_object(Some(last.handle));
            }
            let next = objects.last().map(|o| o.end + 1).unwrap_or(start);
            Ok(Placed { scene, objects, cont: Continue { scene, layer, cursor, next }, warnings })
        })
        .map_err(|e| format!("書き込みを始められませんでした: {e}"))?
}

// ---- 照合 ----

/// シーンにあるテキスト 1 つ
#[derive(Debug, Clone)]
pub struct Telop {
    pub scene: i32,
    pub scene_name: String,
    pub layer: usize,
    pub start: usize,
    pub end: usize,
    /// 表示中のシーンなら Some（フォーカスを移せる）
    pub handle: Option<ObjectHandle>,
    /// 本文（保存形式を戻したもの）
    pub body: String,
}

/// 表示中のシーンのテキストを全部読む。戻り値の 1 つ目はシーンの ID
pub fn read_current_scene() -> Result<(i32, Vec<Telop>), String> {
    ready()?;
    // 編集情報は読み取りセクションに入る前に取る（ColorHistory_H・TextReplace_H と同じ）
    let info = EDIT_HANDLE.get_edit_info();
    let (scene, layer_max) = (info.scene_id, info.layer_max);
    EDIT_HANDLE
        .call_read_section(move |read| {
            let name = read.get_scene_name().unwrap_or_default();
            let mut out = Vec::new();
            for layer in 0..=layer_max {
                for (lf, handle) in read.objects_in_layer(layer) {
                    if let Ok(raw) = read.get_object_effect_item(handle, TEXT_EFFECT, 0, TEXT_ITEM) {
                        out.push(Telop {
                            scene,
                            scene_name: name.clone(),
                            layer: lf.layer,
                            start: lf.start,
                            end: lf.end,
                            handle: Some(handle),
                            body: unescape(&raw),
                        });
                    }
                }
            }
            (scene, out)
        })
        .map_err(|e| format!("読み取りに失敗しました: {e}"))
}

/// テロップの本文を書き換える 1 つ分（本文がずれた行を原稿に合わせる。第 2 段階）
#[derive(Debug, Clone)]
pub struct Rewrite {
    pub handle: ObjectHandle,
    /// 照合したときの本文（保存形式を戻したもの）。書く直前に読み直して違えば書かない
    pub expected: String,
    pub body: String,
}

#[derive(Debug, Default)]
pub struct RewriteReport {
    /// 書けたもの（`Rewrite` の何番目か）
    pub written: Vec<usize>,
    /// 照合の後に本体側で変わっていたので書かなかった
    pub stale: usize,
    pub errors: Vec<String>,
}

/// 本文を書き換える（「原稿の文に直す」を押したときの `call_edit_section` 1 回 = 本体の Undo 1 回分）。書いた後に読み返して確かめる
pub fn rewrite(list: Vec<Rewrite>) -> Result<RewriteReport, String> {
    ready()?;
    EDIT_HANDLE
        .call_edit_section(move |edit| {
            let mut report = RewriteReport::default();
            for (n, w) in list.iter().enumerate() {
                if !edit.object_exists(w.handle) {
                    report.errors.push(format!("{} 個目: オブジェクトがありません", n + 1));
                    continue;
                }
                match edit.get_object_effect_item(w.handle, TEXT_EFFECT, 0, TEXT_ITEM) {
                    Ok(cur) if unescape(&cur) == w.expected => {}
                    Ok(_) => {
                        report.stale += 1;
                        continue;
                    }
                    Err(e) => {
                        report.errors.push(format!("{} 個目: 読めません（{e}）", n + 1));
                        continue;
                    }
                }
                let raw = escape(&w.body);
                if let Err(e) = edit.set_object_effect_item(w.handle, TEXT_EFFECT, 0, TEXT_ITEM, &raw) {
                    report.errors.push(format!("{} 個目: 書けません（{e}）", n + 1));
                    continue;
                }
                match edit.get_object_effect_item(w.handle, TEXT_EFFECT, 0, TEXT_ITEM) {
                    Ok(back) if back == raw => report.written.push(n),
                    Ok(back) => {
                        tracing::warn!("[ManuscriptPanel_H] rewrite {raw:?} read back {back:?}");
                        report.errors.push(format!("{} 個目: 書いた値と読み返しが違います", n + 1));
                    }
                    Err(_) => report.written.push(n),
                }
            }
            report
        })
        .map_err(|e| format!("書き込みを始められませんでした: {e}"))
}

/// 一覧の印をクリックしたとき、そのオブジェクトにフォーカスを移す
pub fn focus(handle: ObjectHandle) -> Result<(), String> {
    ready()?;
    EDIT_HANDLE
        .call_edit_section(move |edit| {
            if edit.object_exists(handle) {
                let _ = edit.set_focus_object(Some(handle));
                true
            } else {
                false
            }
        })
        .map_err(|e| format!("フォーカスを移せませんでした: {e}"))
        .and_then(|ok| if ok { Ok(()) } else { Err("オブジェクトがありません（消されたか、別のシーンです）".into()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_round_trip() {
        let s = "一行目\n二行目の \\ 記号";
        assert_eq!(escape(s), "一行目\\n二行目の \\\\ 記号");
        assert_eq!(unescape(&escape(s)), s);
    }

    #[test]
    fn checks_alias() {
        assert!(check_alias(DEFAULT_ALIAS).is_ok());
        assert!(check_alias("\u{feff}[Object]\n[Object.0]\neffect.name=テキスト\nサイズ=80.00\n").is_ok());
        // 複数のオブジェクト（[0] から始まる）
        assert!(check_alias("[0]\nlayer=1\nframe=0,9\n[0.0]\neffect.name=テキスト\n").is_err());
        // 最初の効果がテキストでない
        assert!(check_alias("[Object]\n[Object.0]\neffect.name=図形\n[Object.1]\neffect.name=テキスト\n").is_err());
        assert!(check_alias("").is_err());
    }

    #[test]
    fn sets_alias_text() {
        let alias = "[Object]\r\n[Object.0]\r\neffect.name=テキスト\r\nサイズ=80.00\r\nテキスト=型の文字\r\n[Object.1]\r\neffect.name=縁取り\r\nテキスト=別の効果\r\n";
        let out = set_alias_text(alias, "原稿の文\\n二行目").unwrap();
        assert_eq!(
            out,
            "[Object]\r\n[Object.0]\r\neffect.name=テキスト\r\nサイズ=80.00\r\nテキスト=原稿の文\\n二行目\r\n[Object.1]\r\neffect.name=縁取り\r\nテキスト=別の効果\r\n"
        );
        // テキストの項目が無い型には足す
        let out = set_alias_text("[Object]\n[Object.0]\neffect.name=テキスト\nサイズ=80.00\n[Object.1]\neffect.name=標準描画\n", "あ").unwrap();
        assert_eq!(out, "[Object]\n[Object.0]\neffect.name=テキスト\nテキスト=あ\nサイズ=80.00\n[Object.1]\neffect.name=標準描画\n");
        let out = set_alias_text("[Object]\n[Object.0]\neffect.name=テキスト\n", "い").unwrap();
        assert_eq!(out, "[Object]\n[Object.0]\neffect.name=テキスト\nテキスト=い\n");
        // 本文の `=` はそのまま
        let out = set_alias_text(DEFAULT_ALIAS, "a=b").unwrap();
        assert!(out.contains("\r\nテキスト=a=b\r\n"));
    }

    #[test]
    fn plans_back_to_back() {
        assert_eq!(plan(100, &[90, 30], 0), [(100, 189), (190, 219)]);
        assert_eq!(plan(0, &[10, 10, 10], 5), [(0, 9), (15, 24), (30, 39)]);
    }

    #[test]
    fn length_by_chars() {
        let l = Length::ByChars { sec_per_char: 0.25, margin_sec: 0.5, min_frames: 25 };
        // 10 文字 × 0.25 + 0.5 = 3 秒 = 90 フレーム
        assert_eq!(l.frames("一二三<#ff0000>四五<#>六七八九十", 30.0), 90);
        // 短い文は最短に
        assert_eq!(l.frames("あ", 30.0), 25);
        assert_eq!(Length::Frames(0).frames("x", 30.0), 1);
    }
}
