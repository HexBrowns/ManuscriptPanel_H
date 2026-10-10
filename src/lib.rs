//! ManuscriptPanel_H（原稿パネル）
//!
//! Markdown の原稿を行ごとに一覧にし、クリックした行を、決めた見た目（型 = エイリアス）のテキストオブジェクトとして
//! カーソル位置に置く。置いた行には印を付け、「照合」でプロジェクトの中身から印を復元する。
//! 仕様は `AI/specifications/20261005_ManuscriptPanel_H_spec.md`。
//!
//! - 本体への書き込みは、行のクリックとボタンのときの `call_edit_section` だけ（`place.rs`）
//! - 原稿の監視は `notify` の中のスレッドだけで、自分ではスレッドを起こさない（`watcher.rs`）。監視はプラグインの `Drop` で外す
//! - 原稿の場所はプロジェクトの中に覚える（保存のときに書き、開いたときに読む）

mod aup2;
mod dialog;
mod gui;
pub mod manuscript;
mod place;
mod settings;
mod watcher;

use std::path::PathBuf;
use std::sync::{Arc, Once};

use aviutl2::generic::GlobalEditHandle;
use aviutl2::AnyResult;
use aviutl2_eframe::egui;
use parking_lot::{Mutex, RwLock};

pub const WINDOW_NAME: &str = "原稿パネル";
/// プロジェクトに原稿の場所を覚えておく名前
const PROJECT_KEY: &str = "manuscript";

pub(crate) static EDIT_HANDLE: GlobalEditHandle = GlobalEditHandle::new();

/// プラグイン本体と UI で共有する状態
#[derive(Default)]
pub struct Shared {
    /// 開いているプロジェクトの `.aup2`（ほかのシーンを照合するため）。未保存なら None
    pub project_path: Option<PathBuf>,
    /// 今開いている原稿（プロジェクトを保存するときに書く）
    pub manuscript: Option<PathBuf>,
    /// 開いたプロジェクトが覚えていた原稿。UI が開いたら下ろす
    pub open_request: Option<PathBuf>,
    pub egui_ctx: Option<egui::Context>,
}

pub type SharedState = Arc<RwLock<Shared>>;

static PIN: Once = Once::new();

/// この DLL をプロセスが終わるまで外させない（待てないスレッド: `notify` の中・シェルのダイアログ）
pub(crate) fn pin_module() {
    PIN.call_once(|| unsafe {
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::System::LibraryLoader::{
            GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN,
        };
        let mut module = HMODULE::default();
        let addr = pin_module as *const () as *const u16;
        if let Err(e) = GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_PIN | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
            windows::core::PCWSTR(addr),
            &mut module,
        ) {
            tracing::warn!("ManuscriptPanel_H: DLL を pin できませんでした: {e}");
        }
    });
}

fn init_logging() {
    use aviutl2::tracing::Level;
    let level = if cfg!(debug_assertions) { Level::DEBUG } else { Level::INFO };
    let _ = aviutl2::tracing_subscriber::fmt()
        .with_max_level(level)
        .event_format(aviutl2::logger::AviUtl2Formatter)
        .with_writer(aviutl2::logger::AviUtl2LogWriter)
        .try_init();
}

#[aviutl2::plugin(GenericPlugin)]
pub struct ManuscriptPanelPlugin {
    window: Mutex<Option<aviutl2_eframe::EframeWindow>>,
    shared: SharedState,
}

impl ManuscriptPanelPlugin {
    fn repaint(&self) {
        if let Some(ctx) = &self.shared.read().egui_ctx {
            ctx.request_repaint();
        }
    }
}

impl aviutl2::generic::GenericPlugin for ManuscriptPanelPlugin {
    fn new(_info: aviutl2::AviUtl2Info) -> AnyResult<Self> {
        init_logging();
        tracing::info!("ManuscriptPanel_H v{} 初期化", env!("CARGO_PKG_VERSION"));
        Ok(Self { window: Mutex::new(None), shared: Arc::new(RwLock::new(Shared::default())) })
    }

    fn plugin_info(&self) -> aviutl2::generic::GenericPluginTable {
        aviutl2::generic::GenericPluginTable {
            name: "ManuscriptPanel_H".to_string(),
            information: format!("ManuscriptPanel_H v{} - 原稿パネル / by HexBrowns", env!("CARGO_PKG_VERSION")),
        }
    }

    fn register(&mut self, registry: &mut aviutl2::generic::HostAppHandle) {
        EDIT_HANDLE.init(registry.create_edit_handle());
        let shared = Arc::clone(&self.shared);
        let window = match aviutl2_eframe::EframeWindow::new(WINDOW_NAME, move |cc, handle| {
            Ok(Box::new(gui::ManuscriptApp::new(cc, handle, shared)))
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("原稿パネルのウィンドウを作れませんでした: {e:#}");
                return;
            }
        };
        match window.handle() {
            Ok(handle) => {
                if let Err(e) = registry.register_window_client(WINDOW_NAME, &handle) {
                    tracing::error!("register_window_client 失敗: {e}");
                }
            }
            Err(e) => tracing::error!("原稿パネルのウィンドウのハンドルを取れませんでした: {e:#}"),
        }
        *self.window.lock() = Some(window);

        // ウィンドウは表示メニューから開く（メニューから ShowWindow しても開かない。2026-10-10 にユーザーの指摘で外した）
    }

    fn on_project_load(&mut self, project: &mut aviutl2::generic::ProjectFile) {
        let remembered = project.get_param_string(PROJECT_KEY).ok().filter(|s| !s.is_empty()).map(PathBuf::from);
        {
            let mut s = self.shared.write();
            s.project_path = project.get_path();
            if remembered.is_some() {
                s.open_request = remembered;
            }
        }
        self.repaint();
    }

    fn on_project_save(&mut self, project: &mut aviutl2::generic::ProjectFile) {
        let manuscript = {
            let mut s = self.shared.write();
            if let Some(p) = project.get_path() {
                s.project_path = Some(p);
            }
            s.manuscript.clone()
        };
        let value = manuscript.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        if let Err(e) = project.set_param_string(PROJECT_KEY, &value) {
            tracing::warn!("ManuscriptPanel_H: プロジェクトに原稿の場所を書けませんでした: {e}");
        }
    }
}

impl Drop for ManuscriptPanelPlugin {
    fn drop(&mut self) {
        // 本体が DLL を外す前に、原稿の監視を外す（中のスレッドは待てないので、DLL は pin してある）
        watcher::stop_all();
    }
}

// 本体が呼ぶ関数（GetCommonPluginTable など）を書き出す（ルール au2-rs-plugin「ビルド」）
aviutl2::register_generic_plugin!(ManuscriptPanelPlugin);
