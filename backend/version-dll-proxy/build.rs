fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    let def_name = if cfg!(feature = "zlib") {
        "defs/zlib.def"
    } else if cfg!(feature = "winmm") {
        "defs/winmm.def"
    } else {
        "version.def"
    };

    let def_path = std::path::PathBuf::from(&manifest_dir).join(def_name);
    println!("cargo:rustc-link-arg={}", def_path.display());
    println!("cargo:rerun-if-changed={}", def_path.display());
}
