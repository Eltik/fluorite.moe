//! Build script for the `backend` crate. Only with the `acl` feature does it compile the C++
//! shim over the vendored ACL decompressor; default builds need no C++ toolchain.
fn main() {
    println!("cargo:rerun-if-changed=vendor/acl/shim/acl_shim.cpp");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_ACL");
    #[cfg(feature = "acl")]
    {
        cc::Build::new()
            .cpp(true)
            .std("c++17")
            .file("vendor/acl/shim/acl_shim.cpp")
            .include("vendor/acl/include")
            .define("ACL_NO_ALLOCATOR_TRACKING", None)
            .flag_if_supported("-w")
            .flag_if_supported("-Wno-missing-template-arg-list-after-template-kw")
            .compile("fluorite_acl");
    }
}
