//! 原稿パネルのウィンドウ（egui）
//!
//! 上: 原稿・型・レイヤー・長さ。中: 見出しごとに折りたためる行の一覧。下: 状態。
//! 行をクリック（設定でダブルクリック）すると置く。左の印（●）をクリックすると、置いたテロップにフォーカスを移す。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use aviutl2::generic::ObjectHandle;
use aviutl2_eframe::{eframe, egui, AviUtl2EframeHandle};

use crate::manuscript::{self, Hit, Item, Manuscript, Unit};
use crate::place::{self, Continue, Length, Piece, PlaceReq, Telop};
use crate::settings::{self, ClickMode, LengthMode, Settings};
use crate::watcher::{self, Dirty};
use crate::{aup2, SharedState};

const YELLOW: egui::Color32 = egui::Color32::from_rgb(235, 200, 90);
const RED: egui::Color32 = egui::Color32::from_rgb(235, 110, 110);
const GREEN: egui::Color32 = egui::Color32::from_rgb(120, 200, 120);

/// 置くもの
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// 一覧の通し番号の行
    Item(usize),
    /// 節の番号（`ms.sections` の何番目か）の見出し
    Heading(usize),
}

enum Action {
    Place(Vec<Target>),
    Focus(ObjectHandle),
    /// 本文がずれたテロップを原稿の文に直す（行の `id()`。空ならずれたもの全部）
    Fix(Vec<String>),
}

pub struct ManuscriptApp {
    _handle: AviUtl2EframeHandle,
    shared: SharedState,
    settings: Settings,
    file: Option<PathBuf>,
    ms: Manuscript,
    parse_error: Option<String>,
    dirty: Dirty,
    types: Vec<(String, PathBuf)>,
    types_skipped: usize,
    /// 照合で読んだテロップと、その後に置いたもの
    telops: Vec<Telop>,
    /// 照合したか（していなければ、このウィンドウで置いたものしか印が付かない）
    checked: bool,
    hits: HashMap<String, Vec<Hit>>,
    cont: Option<Continue>,
    last_placed: Option<String>,
    status: String,
    status_is_error: bool,
    show_settings: bool,
    /// 本文が違う行だけを出す（第 2 段階）
    only_differ: bool,
    /// 前のフレームの終わりに、どこかにフォーカスがあったか・ポップアップが開いていたか（Esc で設定を閉じてよいかの判定）
    busy_last_frame: bool,
}

impl ManuscriptApp {
    pub fn new(cc: &eframe::CreationContext<'_>, handle: AviUtl2EframeHandle, shared: SharedState) -> Self {
        cc.egui_ctx.all_styles_mut(|style| {
            style.visuals = aviutl2_eframe::aviutl2_visuals();
        });
        cc.egui_ctx.set_fonts(aviutl2_eframe::aviutl2_fonts());
        shared.write().egui_ctx = Some(cc.egui_ctx.clone());
        let (types, types_skipped) = place::list_types();
        let mut app = Self {
            _handle: handle,
            shared,
            settings: settings::load(),
            file: None,
            ms: Manuscript::default(),
            parse_error: None,
            dirty: Dirty::default(),
            types,
            types_skipped,
            telops: Vec::new(),
            checked: false,
            hits: HashMap::new(),
            cont: None,
            last_placed: None,
            status: String::new(),
            status_is_error: false,
            show_settings: false,
            only_differ: false,
            busy_last_frame: false,
        };
        // プロジェクトが覚えている原稿が先。無ければ最後に開いたもの
        let first = app.shared.write().open_request.take().or_else(|| app.settings.last_file.clone());
        if let Some(p) = first.filter(|p| p.is_file()) {
            app.open(&cc.egui_ctx, p);
        }
        app
    }

    fn set_status(&mut self, s: impl Into<String>, error: bool) {
        self.status = s.into();
        self.status_is_error = error;
    }

    fn save_settings(&mut self) {
        if let Err(e) = settings::save(&self.settings) {
            self.set_status(e, true);
        }
    }

    fn stem(&self) -> String {
        self.file.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    // ---- 原稿 ----

    fn open(&mut self, ctx: &egui::Context, path: PathBuf) {
        let text = match watcher::read_text(&path) {
            Ok(t) => t,
            Err(e) => {
                self.set_status(e, true);
                return;
            }
        };
        if self.file.as_ref() != Some(&path) {
            // 別の原稿に替えたら、続けて置く記録は捨てる（照合したテロップは残し、新しい原稿で当て直す）
            self.cont = None;
            self.last_placed = None;
        }
        self.file = Some(path.clone());
        self.shared.write().manuscript = Some(path.clone());
        if self.settings.last_file.as_ref() != Some(&path) {
            self.settings.last_file = Some(path.clone());
            self.save_settings();
        }
        self.dirty = Dirty::default();
        let watch_note = match watcher::watch(&path, self.dirty.clone(), ctx.clone()) {
            Ok(()) => String::new(),
            Err(e) => {
                tracing::warn!("[ManuscriptPanel_H] {e}");
                format!("（{e}。保存しても読み直しません）")
            }
        };
        self.parse(&text);
        if self.parse_error.is_none() {
            let n = self.ms.items().count();
            self.set_status(format!("{} を開きました（{n} 行）{watch_note}", self.stem()), !watch_note.is_empty());
        }
    }

    fn reload(&mut self) {
        let Some(path) = self.file.clone() else { return };
        match watcher::read_text(&path) {
            Ok(text) => {
                self.parse(&text);
                if self.parse_error.is_none() {
                    self.set_status(format!("保存されたので読み直しました（{} 行）", self.ms.items().count()), false);
                }
            }
            Err(e) => self.set_status(e, true),
        }
    }

    fn reparse(&mut self) {
        if let Some(path) = self.file.clone() {
            if let Ok(text) = watcher::read_text(&path) {
                self.parse(&text);
            }
        }
    }

    fn parse(&mut self, text: &str) {
        match manuscript::parse(text, &self.settings.options()) {
            Ok(ms) => {
                self.ms = ms;
                self.parse_error = None;
            }
            Err(e) => {
                self.parse_error = Some(e.clone());
                self.set_status(e, true);
            }
        }
        self.rematch();
    }

    fn rematch(&mut self) {
        let bodies: Vec<String> = self.telops.iter().map(|t| t.body.clone()).collect();
        self.hits = manuscript::match_telops(&self.ms, &self.stem(), &bodies);
    }

    // ---- 照合 ----

    fn check(&mut self) {
        let (scene, mut telops) = match place::read_current_scene() {
            Ok(r) => r,
            Err(e) => {
                self.set_status(e, true);
                return;
            }
        };
        let live = telops.len();
        let project = self.shared.read().project_path.clone();
        let note = match project {
            Some(p) => match std::fs::read(&p) {
                Ok(bytes) => {
                    let saved = aup2::parse(&String::from_utf8_lossy(&bytes));
                    let mut n = 0;
                    for t in saved.texts.into_iter().filter(|t| t.scene != scene) {
                        telops.push(Telop {
                            scene: t.scene,
                            scene_name: saved.scenes.get(&t.scene).cloned().unwrap_or_else(|| format!("シーン {}", t.scene)),
                            layer: t.layer,
                            start: t.start,
                            end: t.end,
                            handle: None,
                            body: place::unescape(&t.raw),
                        });
                        n += 1;
                    }
                    format!("・ほかのシーン {n} 個（保存済みの .aup2 から）")
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => format!(
                    "（プロジェクトのファイル {} が見つからないので、ほかのシーンは見ていません）",
                    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                ),
                Err(e) => format!("（保存済みの .aup2 を読めませんでした: {e}）"),
            },
            None => "（プロジェクトが未保存なので、ほかのシーンは見ていません）".to_string(),
        };
        self.telops = telops;
        self.checked = true;
        self.rematch();
        let placed = self.ms.items().filter(|i| self.hits.contains_key(&i.id())).count();
        self.set_status(format!("照合しました: 表示中のシーンのテキスト {live} 個{note}。置いた行 {placed}"), false);
    }

    // ---- 置く ----

    /// 型の名前（`Alias/` からの相対）からエイリアスを読む。空なら本体の初期値
    fn alias_for(&self, name: &str) -> Result<String, String> {
        if name.is_empty() {
            return Ok(place::DEFAULT_ALIAS.to_string());
        }
        let (_, path) = self
            .types
            .iter()
            .find(|(n, _)| n == name)
            .ok_or_else(|| format!("型「{name}」が Alias フォルダに見つかりません"))?;
        let bytes = std::fs::read(path).map_err(|e| format!("型を読めませんでした: {e}"))?;
        let text = String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string();
        place::check_alias(&text)?;
        Ok(text)
    }

    fn length(&self) -> Length {
        match self.settings.length_mode {
            LengthMode::Frames => Length::Frames(self.settings.frames),
            LengthMode::ByChars => Length::ByChars {
                sec_per_char: self.settings.sec_per_char,
                margin_sec: self.settings.margin_sec,
                min_frames: self.settings.min_frames,
            },
        }
    }

    /// 置くものの（印の番号, 本文, 型の名前, 知らせるときの名前）。見出しは型を決めていなければ None
    fn resolve(&self, t: Target) -> Option<(String, String, String, String)> {
        match t {
            Target::Item(k) => {
                let i = self.ms.items().nth(k)?;
                let what = match &i.cut {
                    Some(c) => format!("【{c}】"),
                    None => format!("{} 行目", i.line),
                };
                Some((i.id(), i.text.clone(), self.settings.type_name.clone(), what))
            }
            Target::Heading(n) => {
                let s = self.ms.sections.get(n)?;
                let ty = self.settings.heading_type(s.level)?.clone();
                Some((s.heading_id(), s.title.clone(), ty, format!("見出し「{}」", s.title)))
            }
        }
    }

    /// 節の、まだ置いていないもの（見出しの型を決めていれば見出しも）
    fn unplaced_in_section(&self, n: usize) -> Vec<Target> {
        let base: usize = self.ms.sections[..n].iter().map(|s| s.items.len()).sum();
        let sec = &self.ms.sections[n];
        let mut out = Vec::new();
        if sec.level > 0 && self.settings.heading_type(sec.level).is_some() && !self.hits.contains_key(&sec.heading_id()) {
            out.push(Target::Heading(n));
        }
        out.extend(sec.items.iter().enumerate().filter(|(_, i)| !self.hits.contains_key(&i.id())).map(|(k, _)| Target::Item(base + k)));
        out
    }

    fn place(&mut self, targets: &[Target]) {
        let resolved: Vec<(String, String, String, String)> = targets.iter().filter_map(|&t| self.resolve(t)).collect();
        if resolved.is_empty() {
            return;
        }
        let stem = self.stem();
        let mut pieces = Vec::with_capacity(resolved.len());
        for (id, text, ty, _) in &resolved {
            let alias = match self.alias_for(ty) {
                Ok(a) => a,
                Err(e) => {
                    self.set_status(e, true);
                    return;
                }
            };
            let mark = if self.settings.add_mark { manuscript::mark_comment(&stem, id) } else { String::new() };
            pieces.push(Piece { alias, body: format!("{text}{mark}"), text_for_length: text.clone() });
        }
        let req = PlaceReq {
            gap: if pieces.len() > 1 { self.settings.gap } else { 0 },
            pieces,
            length: self.length(),
            layer: self.settings.fixed_layer.map(|l| l.saturating_sub(1)),
            cont: if self.settings.continuous { self.cont } else { None },
        };
        match place::place(req) {
            Ok(placed) => {
                self.cont = Some(placed.cont);
                let scene_name = self
                    .telops
                    .iter()
                    .find(|t| t.scene == placed.scene && t.handle.is_some())
                    .map(|t| t.scene_name.clone())
                    .unwrap_or_else(|| "表示中のシーン".into());
                for o in &placed.objects {
                    self.telops.push(Telop {
                        scene: placed.scene,
                        scene_name: scene_name.clone(),
                        layer: o.layer,
                        start: o.start,
                        end: o.end,
                        handle: Some(o.handle),
                        body: o.body.clone(),
                    });
                }
                self.rematch();
                // 「次を置く」の起点は行だけ（見出しは数えない）
                let last_item = targets
                    .iter()
                    .filter(|t| matches!(t, Target::Item(_)))
                    .filter_map(|&t| self.resolve(t))
                    .map(|r| r.0)
                    .last();
                if last_item.is_some() {
                    self.last_placed = last_item;
                }
                let first = &placed.objects[0];
                let last = placed.objects.last().unwrap();
                let what = if resolved.len() == 1 { resolved[0].3.clone() } else { format!("{} 個", resolved.len()) };
                let mut msg = format!("{what}を {} レイヤー目の {}〜{} に置きました", first.layer + 1, first.start, last.end);
                if !placed.warnings.is_empty() {
                    msg.push_str(&format!("（{}）", placed.warnings.join("、")));
                }
                self.set_status(msg, !placed.warnings.is_empty());
            }
            Err(e) => self.set_status(e, true),
        }
    }

    /// 本文がずれたテロップ（印で当たって本文が違い、表示中のシーンにあるもの）を原稿の文に直す。`ids` が空なら全部
    fn fix(&mut self, ids: &[String]) {
        let stem = self.stem();
        let mut texts: HashMap<String, String> = HashMap::new();
        for sec in &self.ms.sections {
            if sec.level > 0 {
                texts.insert(sec.heading_id(), sec.title.clone());
            }
            for i in &sec.items {
                texts.insert(i.id(), i.text.clone());
            }
        }
        let mut list = Vec::new();
        let mut telop_of = Vec::new();
        let mut elsewhere = 0;
        for (id, hits) in &self.hits {
            if !ids.is_empty() && !ids.contains(id) {
                continue;
            }
            let Some(text) = texts.get(id) else { continue };
            for h in hits.iter().filter(|h| h.by_mark && !h.same_text) {
                let t = &self.telops[h.telop];
                let Some(handle) = t.handle else {
                    elsewhere += 1;
                    continue;
                };
                list.push(place::Rewrite {
                    handle,
                    expected: t.body.clone(),
                    body: format!("{text}{}", manuscript::mark_comment(&stem, id)),
                });
                telop_of.push(h.telop);
            }
        }
        let note = if elsewhere > 0 {
            format!("（ほかのシーンの {elsewhere} 個は、そのシーンを表示して照合してから直してください）")
        } else {
            String::new()
        };
        if list.is_empty() {
            self.set_status(format!("表示中のシーンに、直すテロップはありません{note}"), elsewhere > 0);
            return;
        }
        let bodies: Vec<String> = list.iter().map(|w| w.body.clone()).collect();
        match place::rewrite(list) {
            Ok(r) => {
                for &n in &r.written {
                    self.telops[telop_of[n]].body = bodies[n].clone();
                }
                self.rematch();
                let mut msg = format!("{} 個を原稿の文に直しました{note}", r.written.len());
                if r.stale > 0 {
                    msg.push_str(&format!("。照合の後に変わっていた {} 個は直していません（照合し直してください）", r.stale));
                }
                if !r.errors.is_empty() {
                    msg.push_str(&format!("。{}", r.errors.join("、")));
                }
                self.set_status(msg, r.stale > 0 || !r.errors.is_empty());
            }
            Err(e) => self.set_status(e, true),
        }
    }

    /// 最後に置いた行の次の、まだ置いていない行（無ければ先頭から探す）
    fn next_unplaced(&self) -> Option<usize> {
        let all: Vec<&Item> = self.ms.items().collect();
        let from = self.last_placed.as_ref().and_then(|id| all.iter().position(|i| &i.id() == id)).map(|p| p + 1).unwrap_or(0);
        (from..all.len()).chain(0..from.min(all.len())).find(|&k| !self.hits.contains_key(&all[k].id()))
    }

    // ---- 描画 ----

    fn render_top(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("開く").on_hover_text("Markdown の原稿を選ぶ（パネルへのドラッグでも開けます）").clicked() {
                let start = self.file.as_ref().and_then(|p| p.parent().map(Path::to_path_buf));
                if let Some(p) = crate::dialog::pick_markdown(start.as_deref()) {
                    let ctx = ui.ctx().clone();
                    self.open(&ctx, p);
                }
            }
            if ui
                .button("照合")
                .on_hover_text("プロジェクトのテキストを読んで、置いた行に印を付け直す（表示中のシーンは今の状態、ほかのシーンは保存済みの .aup2）")
                .clicked()
            {
                self.check();
            }
            match &self.file {
                Some(p) => {
                    let parent = p.parent().and_then(|d| d.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    ui.label(format!("{parent}/{}", p.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()))
                        .on_hover_text(p.display().to_string());
                }
                None => {
                    ui.weak("原稿を開いてください");
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("型");
            let current = if self.settings.type_name.is_empty() { "本体の初期値".to_string() } else { self.settings.type_name.clone() };
            let mut chosen: Option<String> = None;
            egui::ComboBox::from_id_salt("type").selected_text(current).width(200.0).show_ui(ui, |ui| {
                if ui.selectable_label(self.settings.type_name.is_empty(), "本体の初期値").clicked() {
                    chosen = Some(String::new());
                }
                for (name, _) in &self.types {
                    if ui.selectable_label(&self.settings.type_name == name, name).clicked() {
                        chosen = Some(name.clone());
                    }
                }
            });
            if let Some(c) = chosen {
                self.settings.type_name = c;
                self.save_settings();
            }
            let hint = format!(
                "Alias フォルダのテキストのエイリアス（.object）から選ぶ。{} 個{}",
                self.types.len(),
                if self.types_skipped > 0 { format!("（テキスト 1 個でない {} 個は除いた）", self.types_skipped) } else { String::new() }
            );
            if ui.button("↻").on_hover_text(format!("型の一覧を読み直す。{hint}")).clicked() {
                let (t, s) = place::list_types();
                self.types = t;
                self.types_skipped = s;
            }
            ui.separator();
            ui.label("レイヤー");
            let mut fixed = self.settings.fixed_layer.is_some();
            let before = self.settings.fixed_layer;
            egui::ComboBox::from_id_salt("layer_mode")
                .selected_text(if fixed { "固定" } else { "選択中" })
                .width(60.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut fixed, false, "選択中");
                    ui.selectable_value(&mut fixed, true, "固定");
                });
            if fixed {
                let mut n = self.settings.fixed_layer.unwrap_or(1);
                drag_value(ui, &mut n, |d| d.range(1..=999));
                self.settings.fixed_layer = Some(n);
            } else {
                self.settings.fixed_layer = None;
            }
            if before != self.settings.fixed_layer {
                self.save_settings();
            }
        });
        ui.horizontal(|ui| {
            let before = (self.settings.length_mode, self.settings.frames, self.settings.continuous);
            ui.label("長さ");
            ui.radio_value(&mut self.settings.length_mode, LengthMode::Frames, "");
            ui.add_enabled_ui(self.settings.length_mode == LengthMode::Frames, |ui| {
                drag_value(ui, &mut self.settings.frames, |d| d.range(1..=100_000).suffix(" フレーム"));
            });
            ui.radio_value(&mut self.settings.length_mode, LengthMode::ByChars, "文字数で").on_hover_text(format!(
                "文字数 × {} 秒 + {} 秒（最短 {} フレーム）。制御文字は数えない。値は設定で変える",
                self.settings.sec_per_char, self.settings.margin_sec, self.settings.min_frames
            ));
            ui.checkbox(&mut self.settings.continuous, "続けて置く").on_hover_text(
                "直前に置いたテロップの終わりの次のフレームに置く。カーソルを動かしたら、カーソルの位置に置く",
            );
            if before != (self.settings.length_mode, self.settings.frames, self.settings.continuous) {
                self.save_settings();
            }
            ui.separator();
            if ui.button("次を置く").on_hover_text("最後に置いた行の次の、まだ置いていない行を置く").clicked() {
                match self.next_unplaced() {
                    Some(k) => self.place(&[Target::Item(k)]),
                    None => self.set_status("置いていない行はありません", false),
                }
            }
            if ui.button("設定").clicked() {
                self.show_settings = !self.show_settings;
            }
        });
        ui.add_space(2.0);
    }

    fn differ(&self, id: &str) -> bool {
        self.hits.get(id).is_some_and(|h| h.iter().any(|h| !h.same_text))
    }

    fn render_bottom(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        let total = self.ms.items().count();
        let placed = self.ms.items().filter(|i| self.hits.contains_key(&i.id())).count();
        let differ = self.ms.items().filter(|i| self.differ(&i.id())).count()
            + self.ms.sections.iter().filter(|s| s.level > 0 && self.differ(&s.heading_id())).count();
        ui.horizontal(|ui| {
            let mut s = format!("{total} 行・置いた {placed}");
            if self.ms.hidden > 0 {
                s.push_str(&format!("・カット番号の無い {} 行は出していない", self.ms.hidden));
            }
            if !self.checked && self.file.is_some() {
                s.push_str("・未照合");
            }
            ui.weak(s);
            if differ > 0 || self.only_differ {
                ui.toggle_value(&mut self.only_differ, egui::RichText::new(format!("本文が違う {differ}")).color(YELLOW))
                    .on_hover_text("置いたテロップの本文が、今の原稿と違う行だけを出す（原稿だけ、またはテロップだけを直したもの）");
                if self.only_differ && differ > 0 && ui.button("すべて原稿の文に直す").on_hover_text(
                    "表示中のシーンの、本文が違うテロップを原稿の文で置き換える（Undo 1 回分。テロップで足した制御文字も消える）",
                ).clicked()
                {
                    self.fix(&[]);
                }
            }
        });
        if !self.status.is_empty() {
            let color = if self.status_is_error { RED } else { ui.visuals().text_color() };
            ui.add(egui::Label::new(egui::RichText::new(&self.status).color(color)).wrap());
        }
        ui.add_space(2.0);
    }

    fn render_list(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        if let Some(e) = &self.parse_error {
            ui.colored_label(RED, e);
            return;
        }
        if self.file.is_none() {
            ui.weak("「開く」で原稿（.md）を選ぶか、ここへドラッグしてください");
            return;
        }
        if self.ms.items().next().is_none() {
            if self.ms.hidden > 0 {
                ui.colored_label(
                    YELLOW,
                    format!(
                        "カット番号（行頭の【数字】）で始まる行がありません（{} 行を出していません）。\n設定の「カット番号の無い行も出す」をオンにすると、すべての行を出します",
                        self.ms.hidden
                    ),
                );
            } else {
                ui.weak("置ける行がありません");
            }
            return;
        }
        let mut index = 0usize;
        for (n, sec) in self.ms.sections.iter().enumerate() {
            let base = index;
            index += sec.items.len();
            let items = sec.items.iter().enumerate().map(|(k, i)| (base + k, i));
            if sec.level == 0 && sec.title.is_empty() {
                self.render_items(ui, items, actions);
                continue;
            }
            let heading_type = self.settings.heading_type(sec.level);
            if self.only_differ && !self.differ(&sec.heading_id()) && !sec.items.iter().any(|i| self.differ(&i.id())) {
                continue;
            }
            let placed = sec.items.iter().filter(|i| self.hits.contains_key(&i.id())).count();
            // 見出しを置く型を決めていれば、見出しの印も出す
            let mark = match (heading_type, self.hits.get(&sec.heading_id())) {
                (None, _) => "",
                (Some(_), None) => "○ ",
                (Some(_), Some(_)) if self.differ(&sec.heading_id()) => "△ ",
                (Some(_), Some(_)) => "● ",
            };
            let head = if sec.items.is_empty() {
                format!("{mark}{} {}", "#".repeat(sec.level), sec.title)
            } else {
                format!("{mark}{} {}　{placed}/{}", "#".repeat(sec.level), sec.title, sec.items.len())
            };
            let r = egui::CollapsingHeader::new(head).id_salt(("sec", n, sec.line)).default_open(true).show(ui, |ui| {
                self.render_items(ui, items, actions);
            });
            let unplaced = self.unplaced_in_section(n);
            let heading_hits = self.hits.get(&sec.heading_id());
            r.header_response.context_menu(|ui| {
                if let Some(ty) = heading_type {
                    let name = if ty.is_empty() { "本体の初期値" } else { ty.as_str() };
                    if ui.button(format!("見出しを置く（型: {name}）")).clicked() {
                        actions.push(Action::Place(vec![Target::Heading(n)]));
                        ui.close();
                    }
                    if let Some(h) = heading_hits.and_then(|h| h.iter().find_map(|h| self.telops.get(h.telop).and_then(|t| t.handle))) {
                        if ui.button("置いた見出しにフォーカスを移す").clicked() {
                            actions.push(Action::Focus(h));
                            ui.close();
                        }
                    }
                    if self.differ(&sec.heading_id()) && ui.button("見出しのテロップを原稿の文に直す").clicked() {
                        actions.push(Action::Fix(vec![sec.heading_id()]));
                        ui.close();
                    }
                }
                let label = if unplaced.is_empty() {
                    "この節に置いていないものはありません".to_string()
                } else {
                    format!("この節の置いていない {} 個をまとめて置く", unplaced.len())
                };
                if ui.add_enabled(!unplaced.is_empty(), egui::Button::new(label)).clicked() {
                    actions.push(Action::Place(unplaced.clone()));
                    ui.close();
                }
                let differ: Vec<String> = sec.items.iter().map(|i| i.id()).filter(|id| self.differ(id)).collect();
                if !differ.is_empty() && ui.button(format!("この節の本文が違う {} 行を原稿の文に直す", differ.len())).clicked() {
                    actions.push(Action::Fix(differ));
                    ui.close();
                }
            });
            if heading_type.is_none() {
                r.header_response.on_hover_text("右クリック: この節をまとめて置く。見出しの型を設定で決めると、見出しも置ける");
            } else {
                r.header_response.on_hover_text("右クリック: 見出しを置く・この節をまとめて置く");
            }
        }
    }

    fn render_items<'a>(&self, ui: &mut egui::Ui, items: impl Iterator<Item = (usize, &'a Item)>, actions: &mut Vec<Action>) {
        for (k, item) in items {
            let id = item.id();
            let hits = self.hits.get(&id);
            if self.settings.hide_placed && hits.is_some() && !self.only_differ {
                continue;
            }
            if self.only_differ && !self.differ(&id) {
                continue;
            }
            let handle = hits.and_then(|h| h.iter().find_map(|h| self.telops.get(h.telop).and_then(|t| t.handle)));
            let bg = ui.painter().add(egui::Shape::Noop);
            let resp = ui
                .scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
                    ui.horizontal_top(|ui| {
                        let (glyph, color) = match hits {
                            Some(_) if self.differ(&id) => ("●", YELLOW),
                            Some(_) => ("●", GREEN),
                            None => ("○", ui.visuals().weak_text_color()),
                        };
                        let m = ui.add(egui::Label::new(egui::RichText::new(glyph).color(color)).selectable(false).sense(egui::Sense::click()));
                        if hits.is_some() {
                            let m = m.on_hover_text(if handle.is_some() { "クリックで、置いたテロップにフォーカスを移す" } else { "ほかのシーンにあります" });
                            if m.clicked() {
                                if let Some(h) = handle {
                                    actions.push(Action::Focus(h));
                                }
                            }
                        }
                        if let Some(cut) = &item.cut {
                            ui.add(egui::Label::new(egui::RichText::new(format!("【{cut}】")).weak()).selectable(false));
                        }
                        ui.add(egui::Label::new(&item.text).wrap().selectable(false));
                    });
                    if self.settings.show_memos {
                        for memo in &item.memos {
                            ui.horizontal_top(|ui| {
                                ui.add_space(24.0);
                                ui.add(egui::Label::new(egui::RichText::new(format!("※ {memo}")).small().weak()).wrap().selectable(false));
                            });
                        }
                    }
                    // 本文が違うテロップの今の本文（ずれの一覧で、どこが違うかを見る）
                    if self.only_differ {
                        for h in hits.into_iter().flatten().filter(|h| !h.same_text) {
                            if let Some(t) = self.telops.get(h.telop) {
                                let now = manuscript::strip_marks(&t.body);
                                ui.horizontal_top(|ui| {
                                    ui.add_space(24.0);
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(format!("テロップ: {}", now.trim())).small().color(YELLOW))
                                            .wrap()
                                            .selectable(false),
                                    );
                                });
                            }
                        }
                    }
                })
                .response;
            if resp.hovered() {
                ui.painter().set(bg, egui::Shape::rect_filled(resp.rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill));
            }
            let fire = match self.settings.click {
                ClickMode::Single => resp.clicked(),
                ClickMode::Double => resp.double_clicked(),
            };
            resp.context_menu(|ui| {
                if ui.button("この行を置く").clicked() {
                    actions.push(Action::Place(vec![Target::Item(k)]));
                    ui.close();
                }
                if let Some(h) = handle {
                    if ui.button("置いたテロップにフォーカスを移す").clicked() {
                        actions.push(Action::Focus(h));
                        ui.close();
                    }
                }
                if self.differ(&id) && ui.button("テロップを原稿の文に直す").clicked() {
                    actions.push(Action::Fix(vec![id.clone()]));
                    ui.close();
                }
            });
            resp.on_hover_ui(|ui| {
                ui.label(format!("{} 行目", item.line));
                match hits {
                    None => {
                        ui.weak(if self.checked { "置いていない" } else { "置いていない（照合していないので、前に置いたものは分からない）" });
                    }
                    Some(h) => {
                        for hit in h {
                            if let Some(t) = self.telops.get(hit.telop) {
                                let how = if hit.by_mark { "印" } else { "本文が同じ" };
                                let line = format!("{}・{} レイヤー目・{}〜{}（{how}）", t.scene_name, t.layer + 1, t.start, t.end);
                                if hit.same_text {
                                    ui.label(line);
                                } else {
                                    ui.colored_label(YELLOW, format!("{line}: 本文が今の原稿と違う"));
                                }
                            }
                        }
                    }
                }
            });
            if fire && !self.only_differ {
                actions.push(Action::Place(vec![Target::Item(k)]));
            }
        }
    }

    fn render_settings(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let before = self.settings.clone();
        // パネルが低いと見出し（×）が隠れるので、パネルに収めて中をスクロールさせる
        let area = ctx.content_rect().shrink(4.0);
        egui::Window::new("原稿パネルの設定")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .constrain_to(area)
            .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height((area.height() - 40.0).max(80.0)).show(ui, |ui| {
                egui::Grid::new("settings").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("1 テロップの単位");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.settings.unit, Unit::Line, "1 行");
                        ui.radio_value(&mut self.settings.unit, Unit::Paragraph, "空行で区切った段落");
                    });
                    ui.end_row();
                    ui.label("カット番号のパターン");
                    ui.horizontal(|ui| {
                        // 打っている途中の正規表現で読み直さない（Enter・ほかをクリックで確定、Esc で元に戻す）
                        line_edit(ui, "cut_pattern_edit", &mut self.settings.cut_pattern, 160.0).0.on_hover_text(
                            "正規表現。1 番目の括弧が番号になる（無ければ当たった全体）。当たった部分は本文から外す。空なら使わない。\nEnter かほかをクリックで確定、Esc で元に戻す",
                        );
                        if ui.small_button("既定").clicked() {
                            self.settings.cut_pattern = manuscript::DEFAULT_CUT_PATTERN.into();
                        }
                    });
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut self.settings.show_non_cut, "カット番号の無い行も出す");
                    ui.end_row();
                    ui.label("画面メモの行頭");
                    line_edit(ui, "memo_prefix_edit", &mut self.settings.memo_prefix, 40.0).0.on_hover_text(
                        "この文字で始まる行は、直前の行のメモにする（テロップにしない）。空なら使わない。\nEnter かほかをクリックで確定、Esc で元に戻す",
                    );
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut self.settings.show_memos, "メモを出す");
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut self.settings.hide_placed, "置いた行を隠す");
                    ui.end_row();
                    ui.label("置く操作");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.settings.click, ClickMode::Single, "クリック");
                        ui.radio_value(&mut self.settings.click, ClickMode::Double, "ダブルクリック");
                    });
                    ui.end_row();
                    ui.label("印");
                    ui.checkbox(&mut self.settings.add_mark, "本文の末尾に <//原稿:ファイル名:番号//> を付ける")
                        .on_hover_text("表示されないコメント。原稿を直した後や別のパソコンでも、照合で置いた行が分かる");
                    ui.end_row();
                    ui.label("まとめて置くときの間隔");
                    drag_value(ui, &mut self.settings.gap, |d| d.range(0..=10_000).suffix(" フレーム"))
                        .on_hover_text("見出しの右クリックの「まとめて置く」で、テロップの間に空けるフレーム数");
                    ui.end_row();
                    ui.label("見出しの型");
                    ui.vertical(|ui| {
                        if self.settings.heading_types.len() < 6 {
                            self.settings.heading_types.resize(6, None);
                        }
                        for level in 1..=6 {
                            ui.horizontal(|ui| {
                                ui.label("#".repeat(level));
                                let cur = self.settings.heading_types[level - 1].clone();
                                let text = match &cur {
                                    None => "置かない".to_string(),
                                    Some(t) if t.is_empty() => "本体の初期値".to_string(),
                                    Some(t) => t.clone(),
                                };
                                egui::ComboBox::from_id_salt(("heading_type", level)).selected_text(text).width(180.0).show_ui(ui, |ui| {
                                    let slot = &mut self.settings.heading_types[level - 1];
                                    ui.selectable_value(slot, None, "置かない");
                                    ui.selectable_value(slot, Some(String::new()), "本体の初期値");
                                    for (name, _) in &self.types {
                                        ui.selectable_value(slot, Some(name.clone()), name);
                                    }
                                });
                            });
                        }
                    });
                    ui.end_row();
                    ui.label("文字数で決める長さ");
                    ui.horizontal(|ui| {
                        drag_value(ui, &mut self.settings.sec_per_char, |d| d.range(0.01..=5.0).speed(0.01).suffix(" 秒/字"));
                        ui.label("+");
                        drag_value(ui, &mut self.settings.margin_sec, |d| d.range(0.0..=30.0).speed(0.05).suffix(" 秒"));
                        ui.label("最短");
                        drag_value(ui, &mut self.settings.min_frames, |d| d.range(1..=10_000).suffix(" フレーム"));
                    });
                    ui.end_row();
                });
                ui.weak("既定の 0.25 秒/字は読む速さの上限 1 秒 4 文字、最短 25 フレームは 5/6 秒（字幕の基準）");
            });
        });
        self.show_settings = open;
        if self.settings != before {
            let reparse = (self.settings.unit, &self.settings.cut_pattern, &self.settings.memo_prefix, self.settings.show_non_cut)
                != (before.unit, &before.cut_pattern, &before.memo_prefix, before.show_non_cut);
            self.save_settings();
            if reparse {
                self.reparse();
            }
        }
    }
}

/// 数値の欄（ルール au2-rs-plugin「入力の確定と取り消し」）。`update_while_editing(false)` を付けて出す。
/// 打っている途中の値は使わず、Enter かほかをクリックしてフォーカスが外れたときに確定する。入力中の Esc は入力前の値に戻す。
/// ドラッグで変えた値はその場で使う。
///
/// egui 0.36 の `DragValue` は `update_while_editing(false)` でも、Esc の次のフレームで打った文字を値にしてしまう
/// （フォーカスを失ったとみなす期間が 2 フレームあり、2 フレーム目には Esc が押されていないため）。
/// Esc のフレームの値を覚えておき、次のフレームで戻す（参照実装 LayerSilenceCut_H の `setting_value`）
fn drag_value<T>(ui: &mut egui::Ui, value: &mut T, build: impl FnOnce(egui::DragValue<'_>) -> egui::DragValue<'_>) -> egui::Response
where
    T: egui::emath::Numeric + Default + Send + Sync,
{
    let resp = ui.add(build(egui::DragValue::new(value)).update_while_editing(false));
    let key = resp.id.with("escaped");
    let pass = ui.ctx().cumulative_pass_nr();
    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        ui.ctx().data_mut(|d| d.insert_temp(key, (pass, *value)));
    } else if let Some((at, before)) = ui.ctx().data_mut(|d| d.remove_temp::<(u64, T)>(key)) {
        if pass == at + 1 {
            *value = before;
        }
    }
    resp
}

/// 確定してから使う 1 行の入力欄（ルール au2-rs-plugin「入力の確定と取り消し」）。
///
/// 打っている間の文字は欄の中（egui の一時領域）だけに持ち、`value` を書き換えるのは
/// Enter・ほかをクリック・Tab でフォーカスが外れたときだけ。入力中の Esc は、入力を始める前の文字に戻す。
/// 戻り値の bool は、確定して `value` が変わったときだけ true
fn line_edit(ui: &mut egui::Ui, id_salt: &str, value: &mut String, width: f32) -> (egui::Response, bool) {
    // ウィンドウの中で一意な名前から作る（親の Ui の自動の ID は、前に出す部品の数で変わる）
    let id = egui::Id::new(("line_edit", id_salt));
    let key = id.with("pending");
    // 入力中の Esc は取り消し。egui はこのフレームの始めに Esc でフォーカスを外しているので、
    // 「フォーカスが無い + このフレームの Esc」なら、入力中だった文字を描く前に捨てる
    if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) && !ui.ctx().memory(|m| m.has_focus(id)) {
        ui.ctx().data_mut(|d| d.remove_temp::<String>(key));
    }
    // 前のフレームまで入力中だった文字。無ければ今の値を出す
    let mut buf = ui.ctx().data(|d| d.get_temp::<String>(key)).unwrap_or_else(|| value.clone());
    let resp = ui.add(egui::TextEdit::singleline(&mut buf).id(id).desired_width(width));
    let committed = finish_line_edit(ui.ctx(), key, resp.has_focus(), buf, value);
    (resp, committed)
}

/// `line_edit` の後半。フォーカスがある間は入力中の文字を覚え、外れたら確定するか取り消す
fn finish_line_edit(ctx: &egui::Context, key: egui::Id, focused: bool, buf: String, value: &mut String) -> bool {
    if focused {
        ctx.data_mut(|d| d.insert_temp(key, buf));
        return false;
    }
    // 入力中だった文字が残っていれば確定する（Esc で取り消したときは、描く前に捨ててある）
    let was_editing = ctx.data_mut(|d| d.remove_temp::<String>(key)).is_some();
    if !was_editing || buf == *value {
        return false;
    }
    *value = buf;
    true
}

impl eframe::App for ManuscriptApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Esc で設定のウィンドウを閉じるのは、押す前にどこにもフォーカスが無く、ポップアップも開いていなかったときだけ
        // （入力中の Esc は入力を取り消すだけ、コンボボックス・右クリックのメニューの Esc はそれを閉じるだけにする）。
        // egui はこのフレームの始めに Esc でフォーカスを外しているので、前のフレームの終わりの状態で見る
        let esc_closes = ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !self.busy_last_frame;
        // プロジェクトを開いたときに、そのプロジェクトが覚えていた原稿
        let request = self.shared.write().open_request.take();
        if let Some(p) = request {
            if self.file.as_ref() != Some(&p) {
                if p.is_file() {
                    self.open(&ctx, p);
                } else {
                    self.set_status(format!("プロジェクトが覚えていた原稿が見つかりません: {}", p.display()), true);
                }
            }
        }
        // 保存されたら読み直す（変化が止んでから）
        match self.dirty.poll(Instant::now()) {
            Ok(true) => self.reload(),
            Ok(false) => {}
            Err(wait) => ctx.request_repaint_after(wait),
        }
        // パネルへのドラッグ
        let dropped: Option<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf())).filter(|p| !p.as_os_str().is_empty());
        if let Some(p) = dropped {
            self.open(&ctx, p);
        }

        egui::Panel::top("top").show(ui, |ui| self.render_top(ui));
        egui::Panel::bottom("bottom").show(ui, |ui| self.render_bottom(ui));
        let mut actions = Vec::new();
        // 中央パネルは最後に追加する（ルール au2-rs-plugin「egui レイアウト」）
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.render_list(ui, &mut actions));
        });
        if self.show_settings {
            self.render_settings(&ctx);
            if esc_closes {
                self.show_settings = false;
            }
        }
        self.busy_last_frame = ctx.memory(|m| m.focused().is_some()) || ctx.any_popup_open();
        for a in actions {
            match a {
                Action::Place(t) => self.place(&t),
                Action::Fix(ids) => self.fix(&ids),
                Action::Focus(h) => {
                    if let Err(e) = place::focus(h) {
                        self.set_status(e, true);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// テストの 1 フレームの出力を捨てる。テクスチャの差分を片付けずに捨てると、デバッグビルドで epaint の debug_assert
    /// （Dropped TexturesDelta with N unapplied deltas）が落ちる（au2 release の prebuild の cargo test はデバッグビルド）
    fn discard_frame(mut out: egui::FullOutput) {
        out.textures_delta.clear();
    }

    use super::*;

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    /// 1 フレーム回す。戻り値は（確定したか, 欄に出ている文字）
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>, value: &mut String, focus: bool) -> (bool, String) {
        let mut out = (false, String::new());
        let raw = egui::RawInput { events, ..Default::default() };
        discard_frame(ctx.run_ui(raw, |ui| {
            let (resp, committed) = line_edit(ui, "t", value, 100.0);
            if focus {
                resp.request_focus();
            }
            let shown = ui.ctx().data(|d| d.get_temp::<String>(resp.id.with("pending"))).unwrap_or_else(|| value.clone());
            out = (committed, shown);
        }));
        out
    }

    /// フォーカスを当てて "X" を打つところまで
    fn start(ctx: &egui::Context, value: &mut String) {
        frame(ctx, vec![], value, true);
        let (committed, shown) = frame(ctx, vec![egui::Event::Text("X".into())], value, false);
        assert!(!committed);
        assert_eq!(shown, "oldX");
        assert_eq!(value, "old", "打っている途中では値を変えない");
    }

    #[test]
    fn enter_commits() {
        let ctx = egui::Context::default();
        let mut value = "old".to_string();
        start(&ctx, &mut value);
        let (committed, _) = frame(&ctx, vec![key(egui::Key::Enter)], &mut value, false);
        assert!(committed);
        assert_eq!(value, "oldX");
        // 次のフレームで二度確定しない
        assert!(!frame(&ctx, vec![], &mut value, false).0);
    }

    #[test]
    fn escape_reverts() {
        let ctx = egui::Context::default();
        let mut value = "old".to_string();
        start(&ctx, &mut value);
        let (committed, shown) = frame(&ctx, vec![key(egui::Key::Escape)], &mut value, false);
        assert!(!committed);
        assert_eq!(value, "old");
        assert_eq!(shown, "old", "欄の文字も入力前に戻る");
        assert!(!frame(&ctx, vec![], &mut value, false).0);
        assert_eq!(value, "old");
    }

    #[test]
    fn losing_focus_commits() {
        let ctx = egui::Context::default();
        let mut value = "old".to_string();
        start(&ctx, &mut value);
        // ほかをクリックしたのと同じく、フォーカスを外す
        ctx.memory_mut(|m| m.stop_text_input());
        let (committed, _) = frame(&ctx, vec![], &mut value, false);
        assert!(committed);
        assert_eq!(value, "oldX");
    }

    /// 数値の欄を 1 フレーム回す
    fn drag_frame(ctx: &egui::Context, events: Vec<egui::Event>, value: &mut i32, focus: bool) {
        let raw = egui::RawInput { events, ..Default::default() };
        discard_frame(ctx.run_ui(raw, |ui| {
            let resp = drag_value(ui, value, |d| d.range(0..=100));
            if focus {
                resp.request_focus();
            }
        }));
    }

    /// フォーカスを当てて 5 を消し、42 を打つところまで
    fn drag_start(ctx: &egui::Context, value: &mut i32) {
        drag_frame(ctx, vec![], value, true);
        let mut events = vec![key(egui::Key::Backspace); 5];
        events.push(egui::Event::Text("42".into()));
        drag_frame(ctx, events, value, false);
        assert_eq!(*value, 5, "打っている途中では値を変えない");
    }

    #[test]
    fn drag_value_escape_reverts_for_following_frames() {
        let ctx = egui::Context::default();
        let mut value = 5;
        drag_start(&ctx, &mut value);
        drag_frame(&ctx, vec![key(egui::Key::Escape)], &mut value, false);
        assert_eq!(value, 5);
        // egui は Esc の次のフレームで打った文字を値にする。3 フレーム先まで元のままか
        for n in 1..=3 {
            drag_frame(&ctx, vec![], &mut value, false);
            assert_eq!(value, 5, "Esc の {n} フレーム後");
        }
    }

    #[test]
    fn drag_value_enter_commits() {
        let ctx = egui::Context::default();
        let mut value = 5;
        drag_start(&ctx, &mut value);
        drag_frame(&ctx, vec![key(egui::Key::Enter)], &mut value, false);
        assert_eq!(value, 42);
        for _ in 0..3 {
            drag_frame(&ctx, vec![], &mut value, false);
            assert_eq!(value, 42);
        }
    }
}
