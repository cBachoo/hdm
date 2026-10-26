// Embeds Windows version information (product name, description, version from Cargo.toml) in the
// DLL. Unsigned DLLs without it look more suspicious to antivirus reputation heuristics.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set("ProductName", "Umamusume DarkMode")
        .set("FileDescription", "Umamusume DarkMode - Hachimi plugin")
        .set("CompanyName", "cBachoo")
        .set("OriginalFilename", "hachimi_darkmode.dll")
        .set("InternalName", "hachimi_darkmode")
        .set("Comments", "https://github.com/cBachoo/hdm");
    if let Err(e) = res.compile() {
        // Don't fail the build over metadata (e.g. no Windows SDK resource compiler available).
        println!("cargo:warning=could not embed version info: {e}");
    }
}
