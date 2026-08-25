fn main() {
    let build_version = format!("v{}", chrono::Local::now().format("%Y%m%d%H%M%S"));
    println!("cargo:rustc-env=TERMINAL_BUILD_VERSION={build_version}");
    embed_resource::compile("resources/app.rc", embed_resource::NONE);
}
