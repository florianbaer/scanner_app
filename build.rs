fn main() {
    // Finds libsane (e.g. Homebrew's sane-backends) and emits the link flags.
    if pkg_config::probe_library("sane-backends").is_err() {
        // Fallback for Homebrew on Apple Silicon when pkg-config is missing.
        println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
        println!("cargo:rustc-link-lib=sane");
    }
}
