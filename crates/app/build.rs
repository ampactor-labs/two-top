fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        // oboe-sys links Android's static libc++ when Bevy audio pulls cpal in,
        // but with cargo-apk's clang linker the matching ABI archive is not
        // added automatically. Keep this Android-only so desktop links stay
        // unchanged.
        let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("target arch is set");
        let lib_triple = match arch.as_str() {
            "aarch64" => "aarch64-linux-android",
            "arm" => "arm-linux-androideabi",
            "x86" => "i686-linux-android",
            "x86_64" => "x86_64-linux-android",
            other => panic!("unsupported Android arch {other}"),
        };
        let ndk_root = std::env::var("ANDROID_NDK_ROOT")
            .or_else(|_| std::env::var("ANDROID_NDK_HOME"))
            .expect("ANDROID_NDK_ROOT or ANDROID_NDK_HOME is set for Android builds");
        let prebuilt_root = std::path::Path::new(&ndk_root)
            .join("toolchains")
            .join("llvm")
            .join("prebuilt");
        let host_dir = std::fs::read_dir(&prebuilt_root)
            .expect("NDK prebuilt toolchain dir exists")
            .filter_map(Result::ok)
            .find(|entry| entry.file_type().is_ok_and(|ty| ty.is_dir()))
            .expect("NDK prebuilt host toolchain exists")
            .path();
        let abi_lib_dir = host_dir
            .join("sysroot")
            .join("usr")
            .join("lib")
            .join(lib_triple);
        println!("cargo:rustc-link-search=native={}", abi_lib_dir.display());
        println!("cargo:rustc-link-lib=static=c++abi");
    }
}
