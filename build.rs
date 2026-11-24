//! 给 exe 嵌入图标与版本信息（仅 Windows）。

#[cfg(windows)]
fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("app.ico");
    res.set("ProductName", "EncryptoNote");
    res.set("FileDescription", "EncryptoNote - encrypted rich-text notes");
    res.set("LegalCopyright", "");
    // 找不到 rc.exe 时只告警，不阻断构建
    if let Err(e) = res.compile() {
        println!("cargo:warning=icon resource skipped: {e}");
    }
}

#[cfg(not(windows))]
fn main() {}
