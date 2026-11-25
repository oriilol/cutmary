fn main() {
    println!("cargo:rerun-if-changed=assets");
    glib_build_tools::compile_resources(&["."], "resources.gresource.xml", "cutmary.gresource");
}
