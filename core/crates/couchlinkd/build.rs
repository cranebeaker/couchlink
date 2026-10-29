// Puts the version information (company, product, version) into couchlinkd.exe.
fn main() {
    println!("cargo:rerun-if-changed=couchlinkd.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("couchlinkd.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
}
