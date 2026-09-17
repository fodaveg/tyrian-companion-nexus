//! Makes `-lstdc++` resolve to the static archive when cross-building for
//! `x86_64-pc-windows-gnu`.
//!
//! Nexus loads the addon with `LoadLibrary`, and Windows (or Wine) resolves its dependencies
//! from the directory of `Gw2-64.exe`, not from `addons/`. Linked the default way, this DLL
//! ends up importing `libstdc++-6.dll`, which is not there, and Nexus drops it with
//! `Failed LoadLibrary ... Error Code 126 : Module not found` — the addon simply never shows
//! up in game. The C++ runtime comes in through `arcdps-imgui-sys`, which `nexus` depends on
//! unconditionally and which compiles ImGui as C++, even though this addon never calls ImGui.
//!
//! `-static-libstdc++` in `.cargo/config.toml` does not fix it on its own: that is a driver
//! option, and it only governs the `-lstdc++` the driver adds by itself. Here the flag is
//! emitted by `arcdps-imgui-sys` and lands in the middle of the link line, ahead of every
//! `-C link-arg`, so `-Wl,-Bstatic` arrives too late to cover it (measured with
//! `-C link-arg=-###`: `-lstdc++` sits at argument 268, the `-Bstatic` block at 291).
//!
//! What does work is the library search path. `ld` walks `-L` directories in order and only
//! falls back to the toolchain's own at the end, and the toolchain's directory is the one
//! holding both `libstdc++.dll.a` and `libstdc++.a`. Putting a directory that holds only the
//! static archive ahead of it makes `-lstdc++` resolve there, whatever order the flag itself
//! appears in.
use std::path::PathBuf;
use std::process::Command;

fn main() {
	println!("cargo:rerun-if-changed=build.rs");
	let target = std::env::var("TARGET").unwrap_or_default();
	if !target.ends_with("windows-gnu") {
		return;
	}

	let compiler = std::env::var("CXX_x86_64_pc_windows_gnu")
		.unwrap_or_else(|_| "x86_64-w64-mingw32-g++".to_string());
	let output = Command::new(&compiler)
		.arg("-print-file-name=libstdc++.a")
		.output()
		.unwrap_or_else(|error| panic!("could not run {compiler}: {error}"));
	let reported = String::from_utf8_lossy(&output.stdout).trim().to_string();
	let archive = PathBuf::from(&reported);
	// `-print-file-name` echoes the bare name back when it cannot find the file, so a path
	// that does not exist means the C++ cross toolchain is missing, not that it is elsewhere.
	assert!(
		archive.is_file(),
		"{compiler} did not find libstdc++.a (it reported {reported:?}); install the mingw-w64 \
		 C++ cross toolchain (on Fedora, mingw64-gcc-c++)",
	);

	let staging = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"))
		.join("static-libstdc++");
	std::fs::create_dir_all(&staging).expect("could not create the staging directory");
	let staged = staging.join("libstdc++.a");
	// Copy rather than symlink: the search path has to hold a real archive for `ld`, and a
	// copy survives a toolchain upgrade that replaces the original out from under us.
	std::fs::copy(&archive, &staged)
		.unwrap_or_else(|error| panic!("could not copy {archive:?} to {staged:?}: {error}"));
	println!("cargo:rustc-link-search=native={}", staging.display());
	println!("cargo:rerun-if-changed={}", archive.display());
}
