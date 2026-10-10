//! 原稿を選ぶ Windows のダイアログ（BackupDiff_H の `dialog.rs` と同じ作り）。
//!
//! ボタンを押したときに UI のスレッドで開くモーダル。シェルが中で起こすスレッドはこちらから待てないので、DLL を pin する。

use std::path::{Path, PathBuf};

use windows::core::{w, HSTRING};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// Markdown の原稿を選ぶ。`start` があればそのフォルダから開く。取り消したら `None`
pub fn pick_markdown(start: Option<&Path>) -> Option<PathBuf> {
    crate::pin_module();
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let out = unsafe {
        (|| -> Option<PathBuf> {
            let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let options = dialog.GetOptions().ok()?;
            dialog.SetOptions(options | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM).ok()?;
            let filter = [
                COMDLG_FILTERSPEC { pszName: w!("Markdown"), pszSpec: w!("*.md;*.markdown;*.txt") },
                COMDLG_FILTERSPEC { pszName: w!("すべてのファイル"), pszSpec: w!("*.*") },
            ];
            dialog.SetFileTypes(&filter).ok()?;
            let _ = dialog.SetTitle(w!("原稿を開く"));
            if let Some(dir) = start.filter(|d| d.is_dir()) {
                if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(dir.as_os_str()), None) {
                    let _ = dialog.SetFolder(&item);
                }
            }
            dialog.Show(Some(GetForegroundWindow())).ok()?;
            let item = dialog.GetResult().ok()?;
            let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let path = name.to_string().ok();
            CoTaskMemFree(Some(name.0 as *const _));
            path.map(PathBuf::from)
        })()
    };
    if hr.is_ok() {
        unsafe { CoUninitialize() };
    }
    out
}
