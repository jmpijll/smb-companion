fn main() {
    println!("cargo:rerun-if-changed=assets/icon/smb-lol.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/smb-lol.ico")
            .set("ProductName", "SMB LoL Companion")
            .set("FileDescription", "SMB LoL Companion - runes & spells")
            .set("CompanyName", "SMB - Sexy Maar Bescheiden")
            .set("LegalCopyright", "MIT License. Not endorsed by Riot Games.")
            .set("OriginalFilename", "smb-lol.exe");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {e}");
        }
    }
}
