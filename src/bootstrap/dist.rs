// Copyright 2016 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Implementation of the various distribution aspects of the compiler.
//!
//! This module is responsible for creating tarballs of the standard library,
//! compiler, and documentation. This ends up being what we distribute to
//! everyone as well.
//!
//! No tarball is actually created literally in this file, but rather we shell
//! out to `rust-installer` still. This may one day be replaced with bits and
//! pieces of `rustup.rs`!

use std::env;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{PathBuf, Path};
use std::process::{Command, Stdio};

use build_helper::output;

use {Build, Compiler, Mode};
use channel;
use util::{cp_r, libdir, is_dylib, cp_filtered, copy, exe};

pub fn pkgname(build: &Build, component: &str) -> String {
    if component == "cargo" {
        format!("{}-{}", component, build.cargo_package_vers())
    } else if component == "rls" {
        format!("{}-{}", component, build.rls_package_vers())
    } else {
        assert!(component.starts_with("rust"));
        format!("{}-{}", component, build.rust_package_vers())
    }
}

fn distdir(build: &Build) -> PathBuf {
    build.out.join("dist")
}

pub fn tmpdir(build: &Build) -> PathBuf {
    build.out.join("tmp/dist")
}

fn rust_installer(build: &Build) -> Command {
    build.tool_cmd(&Compiler::new(0, &build.config.build), "rust-installer")
}

/// Builds the `rust-docs` installer component.
///
/// Slurps up documentation from the `stage`'s `host`.
pub fn docs(build: &Build, stage: u32, host: &str) {
    println!("Dist docs stage{} ({})", stage, host);
    if !build.config.docs {
        println!("\tskipping - docs disabled");
        return
    }

    let name = pkgname(build, "rust-docs");
    let image = tmpdir(build).join(format!("{}-{}-image", name, host));
    let _ = fs::remove_dir_all(&image);

    let dst = image.join("share/doc/rust/html");
    t!(fs::create_dir_all(&dst));
    let src = build.out.join(host).join("doc");
    cp_r(&src, &dst);

    let mut cmd = rust_installer(build);
    cmd.arg("generate")
       .arg("--product-name=Rust-Documentation")
       .arg("--rel-manifest-dir=rustlib")
       .arg("--success-message=Rust-documentation-is-installed.")
       .arg("--image-dir").arg(&image)
       .arg("--work-dir").arg(&tmpdir(build))
       .arg("--output-dir").arg(&distdir(build))
       .arg(format!("--package-name={}-{}", name, host))
       .arg("--component-name=rust-docs")
       .arg("--legacy-manifest-dirs=rustlib,cargo")
       .arg("--bulk-dirs=share/doc/rust/html");
    build.run(&mut cmd);
    t!(fs::remove_dir_all(&image));

    // As part of this step, *also* copy the docs directory to a directory which
    // buildbot typically uploads.
    if host == build.config.build {
        let dst = distdir(build).join("doc").join(build.rust_package_vers());
        t!(fs::create_dir_all(&dst));
        cp_r(&src, &dst);
    }
}

fn find_files(files: &[&str], path: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = Vec::new();

    for file in files {
        let file_path =
            path.iter()
                .map(|dir| dir.join(file))
                .find(|p| p.exists());

        if let Some(file_path) = file_path {
            found.push(file_path);
        } else {
            panic!("Could not find '{}' in {:?}", file, path);
        }
    }

    found
}

fn make_win_dist(rust_root: &Path, plat_root: &Path, target_triple: &str, build: &Build) {
    //Ask gcc where it keeps its stuff
    let mut cmd = Command::new(build.cc(target_triple));
    cmd.arg("-print-search-dirs");
    build.run_quiet(&mut cmd);
    let gcc_out =
        String::from_utf8(
                cmd
                .output()
                .expect("failed to execute gcc")
                .stdout).expect("gcc.exe output was not utf8");

    let mut bin_path: Vec<_> =
        env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .collect();
    let mut lib_path = Vec::new();

    for line in gcc_out.lines() {
        let idx = line.find(':').unwrap();
        let key = &line[..idx];
        let trim_chars: &[_] = &[' ', '='];
        let value =
            line[(idx + 1)..]
                .trim_left_matches(trim_chars)
                .split(';')
                .map(|s| PathBuf::from(s));

        if key == "programs" {
            bin_path.extend(value);
        } else if key == "libraries" {
            lib_path.extend(value);
        }
    }

    let target_tools = vec!["gcc.exe", "ld.exe", "ar.exe", "dlltool.exe", "libwinpthread-1.dll"];
    let mut rustc_dlls = vec!["libstdc++-6.dll", "libwinpthread-1.dll"];
    if target_triple.starts_with("i686-") {
        rustc_dlls.push("libgcc_s_dw2-1.dll");
    } else {
        rustc_dlls.push("libgcc_s_seh-1.dll");
    }

    let target_libs = vec![ //MinGW libs
        "libgcc.a",
        "libgcc_eh.a",
        "libgcc_s.a",
        "libm.a",
        "libmingw32.a",
        "libmingwex.a",
        "libstdc++.a",
        "libiconv.a",
        "libmoldname.a",
        "libpthread.a",
        //Windows import libs
        "libadvapi32.a",
        "libbcrypt.a",
        "libcomctl32.a",
        "libcomdlg32.a",
        "libcrypt32.a",
        "libgdi32.a",
        "libimagehlp.a",
        "libiphlpapi.a",
        "libkernel32.a",
        "libmsvcrt.a",
        "libodbc32.a",
        "libole32.a",
        "liboleaut32.a",
        "libopengl32.a",
        "libpsapi.a",
        "librpcrt4.a",
        "libsetupapi.a",
        "libshell32.a",
        "libuser32.a",
        "libuserenv.a",
        "libuuid.a",
        "libwinhttp.a",
        "libwinmm.a",
        "libwinspool.a",
        "libws2_32.a",
        "libwsock32.a",
    ];

    //Find mingw artifacts we want to bundle
    let target_tools = find_files(&target_tools, &bin_path);
    let rustc_dlls = find_files(&rustc_dlls, &bin_path);
    let target_libs = find_files(&target_libs, &lib_path);

    fn copy_to_folder(src: &Path, dest_folder: &Path) {
        let file_name = src.file_name().unwrap().to_os_string();
        let dest = dest_folder.join(file_name);
        copy(src, &dest);
    }

    //Copy runtime dlls next to rustc.exe
    let dist_bin_dir = rust_root.join("bin/");
    fs::create_dir_all(&dist_bin_dir).expect("creating dist_bin_dir failed");
    for src in rustc_dlls {
        copy_to_folder(&src, &dist_bin_dir);
    }

    //Copy platform tools to platform-specific bin directory
    let target_bin_dir = plat_root.join("lib").join("rustlib").join(target_triple).join("bin");
    fs::create_dir_all(&target_bin_dir).expect("creating target_bin_dir failed");
    for src in target_tools {
        copy_to_folder(&src, &target_bin_dir);
    }

    //Copy platform libs to platform-specific lib directory
    let target_lib_dir = plat_root.join("lib").join("rustlib").join(target_triple).join("lib");
    fs::create_dir_all(&target_lib_dir).expect("creating target_lib_dir failed");
    for src in target_libs {
        copy_to_folder(&src, &target_lib_dir);
    }
}

/// Build the `rust-mingw` installer component.
///
/// This contains all the bits and pieces to run the MinGW Windows targets
/// without any extra installed software (e.g. we bundle gcc, libraries, etc).
/// Currently just shells out to a python script, but that should be rewritten
/// in Rust.
pub fn mingw(build: &Build, host: &str) {
    println!("Dist mingw ({})", host);
    let name = pkgname(build, "rust-mingw");
    let image = tmpdir(build).join(format!("{}-{}-image", name, host));
    let _ = fs::remove_dir_all(&image);
    t!(fs::create_dir_all(&image));

    // The first argument is a "temporary directory" which is just
    // thrown away (this contains the runtime DLLs included in the rustc package
    // above) and the second argument is where to place all the MinGW components
    // (which is what we want).
    make_win_dist(&tmpdir(build), &image, host, &build);

    let mut cmd = rust_installer(build);
    cmd.arg("generate")
       .arg("--product-name=Rust-MinGW")
       .arg("--rel-manifest-dir=rustlib")
       .arg("--success-message=Rust-MinGW-is-installed.")
       .arg("--image-dir").arg(&image)
       .arg("--work-dir").arg(&tmpdir(build))
       .arg("--output-dir").arg(&distdir(build))
       .arg(format!("--package-name={}-{}", name, host))
       .arg("--component-name=rust-mingw")
       .arg("--legacy-manifest-dirs=rustlib,cargo");
    build.run(&mut cmd);
    t!(fs::remove_dir_all(&image));
}

/// Creates the `rustc` installer component.
pub fn rustc(build: &Build, stage: u32, host: &str) {
    println!("Dist rustc stage{} ({})", stage, host);
    let name = pkgname(build, "rustc");
    let image = tmpdir(build).join(format!("{}-{}-image", name, host));
    let _ = fs::remove_dir_all(&image);
    let overlay = tmpdir(build).join(format!("{}-{}-overlay", name, host));
    let _ = fs::remove_dir_all(&overlay);

    // Prepare the rustc "image", what will actually end up getting installed
    prepare_image(build, stage, host, &image);

    // Prepare the overlay which is part of the tarball but won't actually be
    // installed
    let cp = |file: &str| {
        install(&build.src.join(file), &overlay, 0o644);
    };
    cp("COPYRIGHT");
    cp("LICENSE-APACHE");
    cp("LICENSE-MIT");
    cp("README.md");
    // tiny morsel of metadata is used by rust-packaging
    let version = build.rust_version();
    t!(t!(File::create(overlay.join("version"))).write_all(version.as_bytes()));

    // On MinGW we've got a few runtime DLL dependencies that we need to
    // include. The first argument to this script is where to put these DLLs
    // (the image we're creating), and the second argument is a junk directory
    // to ignore all other MinGW stuff the script creates.
    //
    // On 32-bit MinGW we're always including a DLL which needs some extra
    // licenses to distribute. On 64-bit MinGW we don't actually distribute
    // anything requiring us to distribute a license, but it's likely the
    // install will *also* include the rust-mingw package, which also needs
    // licenses, so to be safe we just include it here in all MinGW packages.
    if host.contains("pc-windows-gnu") {
        make_win_dist(&image, &tmpdir(build), host, build);

        let dst = image.join("share/doc");
        t!(fs::create_dir_all(&dst));
        cp_r(&build.src.join("src/etc/third-party"), &dst);
    }

    // Finally, wrap everything up in a nice tarball!
    let mut cmd = rust_installer(build);
    cmd.arg("generate")
       .arg("--product-name=Rust")
       .arg("--rel-manifest-dir=rustlib")
       .arg("--success-message=Rust-is-ready-to-roll.")
       .arg("--image-dir").arg(&image)
       .arg("--work-dir").arg(&tmpdir(build))
       .arg("--output-dir").arg(&distdir(build))
       .arg("--non-installed-overlay").arg(&overlay)
       .arg(format!("--package-name={}-{}", name, host))
       .arg("--component-name=rustc")
       .arg("--legacy-manifest-dirs=rustlib,cargo");
    build.run(&mut cmd);
    t!(fs::remove_dir_all(&image));
    t!(fs::remove_dir_all(&overlay));

    fn prepare_image(build: &Build, stage: u32, host: &str, image: &Path) {
        let src = build.sysroot(&Compiler::new(stage, host));
        let libdir = libdir(host);

        // Copy rustc/rustdoc binaries
        t!(fs::create_dir_all(image.join("bin")));
        cp_r(&src.join("bin"), &image.join("bin"));

        // Copy runtime DLLs needed by the compiler
        if libdir != "bin" {
            for entry in t!(src.join(libdir).read_dir()).map(|e| t!(e)) {
                let name = entry.file_name();
                if let Some(s) = name.to_str() {
                    if is_dylib(s) {
                        install(&entry.path(), &image.join(libdir), 0o644);
                    }
                }
            }
        }

        // Man pages
        t!(fs::create_dir_all(image.join("share/man/man1")));
        cp_r(&build.src.join("man"), &image.join("share/man/man1"));

        // Debugger scripts
        debugger_scripts(build, &image, host);

        // Misc license info
        let cp = |file: &str| {
            install(&build.src.join(file), &image.join("share/doc/rust"), 0o644);
        };
        cp("COPYRIGHT");
        cp("LICENSE-APACHE");
        cp("LICENSE-MIT");
        cp("README.md");
    }
}

/// Copies debugger scripts for `host` into the `sysroot` specified.
pub fn debugger_scripts(build: &Build,
                        sysroot: &Path,
                        host: &str) {
    let cp_debugger_script = |file: &str| {
        let dst = sysroot.join("lib/rustlib/etc");
        t!(fs::create_dir_all(&dst));
        install(&build.src.join("src/etc/").join(file), &dst, 0o644);
    };
    if host.contains("windows-msvc") {
        // windbg debugger scripts
        install(&build.src.join("src/etc/rust-windbg.cmd"), &sysroot.join("bin"),
            0o755);

        cp_debugger_script("natvis/libcore.natvis");
        cp_debugger_script("natvis/libcollections.natvis");
    } else {
        cp_debugger_script("debugger_pretty_printers_common.py");

        // gdb debugger scripts
        install(&build.src.join("src/etc/rust-gdb"), &sysroot.join("bin"),
                0o755);

        cp_debugger_script("gdb_load_rust_pretty_printers.py");
        cp_debugger_script("gdb_rust_pretty_printing.py");

        // lldb debugger scripts
        install(&build.src.join("src/etc/rust-lldb"), &sysroot.join("bin"),
                0o755);

        cp_debugger_script("lldb_rust_formatters.py");
    }
}

/// Creates the `rust-std` installer component as compiled by `compiler` for the
/// target `target`.
pub fn std(build: &Build, compiler: &Compiler, target: &str) {
    println!("Dist std stage{} ({} -> {})", compiler.stage, compiler.host,
             target);

    // The only true set of target libraries came from the build triple, so
    // let's reduce redundant work by only producing archives from that host.
    if compiler.host != build.config.build {
        println!("\tskipping, not a build host");
        return
    }

    let name = pkgname(build, "rust-std");
    let image = tmpdir(build).join(format!("{}-{}-image", name, target));
    let _ = fs::remove_dir_all(&image);

    let dst = image.join("lib/rustlib").join(target);
    t!(fs::create_dir_all(&dst));
    let src = build.sysroot(compiler).join("lib/rustlib");
    cp_r(&src.join(target), &dst);

    let mut cmd = rust_installer(build);
    cmd.arg("generate")
       .arg("--product-name=Rust")
       .arg("--rel-manifest-dir=rustlib")
       .arg("--success-message=std-is-standing-at-the-ready.")
       .arg("--image-dir").arg(&image)
       .arg("--work-dir").arg(&tmpdir(build))
       .arg("--output-dir").arg(&distdir(build))
       .arg(format!("--package-name={}-{}", name, target))
       .arg(format!("--component-name=rust-std-{}", target))
       .arg("--legacy-manifest-dirs=rustlib,cargo");
    build.run(&mut cmd);
    t!(fs::remove_dir_all(&image));
}

/// The path to the complete rustc-src tarball
pub fn rust_src_location(build: &Build) -> PathBuf {
    let plain_name = format!("rustc-{}-src", build.rust_package_vers());
    distdir(build).join(&format!("{}.tar.gz", plain_name))
}

/// The path to the rust-src component installer
pub fn rust_src_installer(build: &Build) -> PathBuf {
    let name = pkgname(build, "rust-src");
    distdir(build).join(&format!("{}.tar.gz", name))
}

/// Creates a tarball of save-analysis metadata, if available.
pub fn analysis(build: &Build, compiler: &Compiler, target: &str) {
    assert!(build.config.extended);
    println!("Dist analysis");

    if compiler.host != build.config.build {
        println!("\tskipping, not a build host");
        return;
    }

    // Package save-analysis from stage1 if not doing a full bootstrap, as the
    // stage2 artifacts is simply copied from stage1 in that case.
    let compiler = if build.force_use_stage1(compiler, target) {
        Compiler::new(1, compiler.host)
    } else {
        compiler.clone()
    };

    let name = pkgname(build, "rust-analysis");
    let image = tmpdir(build).join(format!("{}-{}-image", name, target));

    let src = build.stage_out(&compiler, Mode::Libstd).join(target).join("release").join("deps");

    let image_src = src.join("save-analysis");
    let dst = image.join("lib/rustlib").join(target).join("analysis");
    t!(fs::create_dir_all(&dst));
    println!("image_src: {:?}, dst: {:?}", image_src, dst);
    cp_r(&image_src, &dst);

    let mut cmd = rust_installer(build);
    cmd.arg("generate")
       .arg("--product-name=Rust")
       .arg("--rel-manifest-dir=rustlib")
       .arg("--success-message=save-analysis-saved.")
       .arg("--image-dir").arg(&image)
       .arg("--work-dir").arg(&tmpdir(build))
       .arg("--output-dir").arg(&distdir(build))
       .arg(format!("--package-name={}-{}", name, target))
       .arg(format!("--component-name=rust-analysis-{}", target))
       .arg("--legacy-manifest-dirs=rustlib,cargo");
    build.run(&mut cmd);
    t!(fs::remove_dir_all(&image));
}

fn copy_src_dirs(build: &Build, src_dirs: &[&str], exclude_dirs: &[&str], dst_dir: &Path) {
    fn filter_fn(exclude_dirs: &[&str], dir: &str, path: &Path) -> bool {
        let spath = match path.to_str() {
            Some(path) => path,
            None => return false,
        };
        if spath.ends_with("~") || spath.ends_with(".pyc") {
            return false
        }
        if spath.contains("llvm/test") || spath.contains("llvm\\test") {
            if spath.ends_with(".ll") ||
               spath.ends_with(".td") ||
               spath.ends_with(".s") {
                return false
            }
        }

        let full_path = Path::new(dir).join(path);
        if exclude_dirs.iter().any(|excl| full_path == Path::new(excl)) {
            return false;
        }

        let excludes = [
            "CVS", "RCS", "SCCS", ".git", ".gitignore", ".gitmodules",
            ".gitattributes", ".cvsignore", ".svn", ".arch-ids", "{arch}",
            "=RELEASE-ID", "=meta-update", "=update", ".bzr", ".bzrignore",
            ".bzrtags", ".hg", ".hgignore", ".hgrags", "_darcs",
        ];
        !path.iter()
             .map(|s| s.to_str().unwrap())
             .any(|s| excludes.contains(&s))
    }

    // Copy the directories using our filter
    for item in src_dirs {
        let dst = &dst_dir.join(item);
        t!(fs::create_dir_all(dst));
        cp_filtered(&build.src.join(item), dst, &|path| filter_fn(exclude_dirs, item, path));
    }
}

/// Creates the `rust-src` installer component
pub fn rust_src(build: &Build) {
    println!("Dist src");

    let name = pkgname(build, "rust-src");
    let image = tmpdir(build).join(format!("{}-image", name));
    let _ = fs::remove_dir_all(&image);

    let dst = image.join("lib/rustlib/src");
    let dst_src = dst.join("rust");
    t!(fs::create_dir_all(&dst_src));

    // This is the reduced set of paths which will become the rust-src component
    // (essentially libstd and all of its path dependencies)
    let std_src_dirs = [
        "src/build_helper",
        "src/liballoc",
        "src/liballoc_jemalloc",
        "src/liballoc_system",
        "src/libbacktrace",
        "src/libcollections",
        "src/libcompiler_builtins",
        "src/libcore",
        "src/liblibc",
        "src/libpanic_abort",
        "src/libpanic_unwind",
        "src/librand",
        "src/librustc_asan",
        "src/librustc_lsan",
        "src/librustc_msan",
        "src/librustc_tsan",
        "src/libstd",
        "src/libstd_unicode",
        "src/libunwind",
        "src/rustc/libc_shim",
        "src/libtest",
        "src/libterm",
        "src/libgetopts",
        "src/compiler-rt",
        "src/jemalloc",
    ];
    let std_src_dirs_exclude = [
        "src/compiler-rt/test",
        "src/jemalloc/test/unit",
    ];

    copy_src_dirs(build, &std_src_dirs[..], &std_src_dirs_exclude[..], &dst_src);

    // Create source tarball in rust-installer format
    let mut cmd = rust_installer(build);
    cmd.arg("generate")
       .arg("--product-name=Rust")
       .arg("--rel-manifest-dir=rustlib")
       .arg("--success-message=Awesome-Source.")
       .arg("--image-dir").arg(&image)
       .arg("--work-dir").arg(&tmpdir(build))
       .arg("--output-dir").arg(&distdir(build))
       .arg(format!("--package-name={}", name))
       .arg("--component-name=rust-src")
       .arg("--legacy-manifest-dirs=rustlib,cargo");
    build.run(&mut cmd);

    t!(fs::remove_dir_all(&image));
}

const CARGO_VENDOR_VERSION: &'static str = "0.1.4";

/// Creates the plain source tarball
pub fn plain_source_tarball(build: &Build) {
    println!("Create plain source tarball");

    // Make sure that the root folder of tarball has the correct name
    let plain_name = format!("{}-src", pkgname(build, "rustc"));
    let plain_dst_src = tmpdir(build).join(&plain_name);
    let _ = fs::remove_dir_all(&plain_dst_src);
    t!(fs::create_dir_all(&plain_dst_src));

    // This is the set of root paths which will become part of the source package
    let src_files = [
        "COPYRIGHT",
        "LICENSE-APACHE",
        "LICENSE-MIT",
        "CONTRIBUTING.md",
        "README.md",
        "RELEASES.md",
        "configure",
        "x.py",
    ];
    let src_dirs = [
        "man",
        "src",
    ];

    copy_src_dirs(build, &src_dirs[..], &[], &plain_dst_src);

    // Copy the files normally
    for item in &src_files {
        copy(&build.src.join(item), &plain_dst_src.join(item));
    }

    // Create the version file
    write_file(&plain_dst_src.join("version"), build.rust_version().as_bytes());

    // If we're building from git sources, we need to vendor a complete distribution.
    if build.src_is_git {
        // Get cargo-vendor installed, if it isn't already.
        let mut has_cargo_vendor = false;
        let mut cmd = Command::new(&build.cargo);
        for line in output(cmd.arg("install").arg("--list")).lines() {
            has_cargo_vendor |= line.starts_with("cargo-vendor ");
        }
        if !has_cargo_vendor {
            let mut cmd = Command::new(&build.cargo);
            cmd.arg("install")
               .arg("--force")
               .arg("--debug")
               .arg("--vers").arg(CARGO_VENDOR_VERSION)
               .arg("cargo-vendor")
               .env("RUSTC", &build.rustc);
            build.run(&mut cmd);
        }

        // Vendor all Cargo dependencies
        let mut cmd = Command::new(&build.cargo);
        cmd.arg("vendor")
           .current_dir(&plain_dst_src.join("src"));
        build.run(&mut cmd);
    }

    // Create plain source tarball
    let mut tarball = rust_src_location(build);
    tarball.set_extension(""); // strip .gz
    tarball.set_extension(""); // strip .tar
    if let Some(dir) = tarball.parent() {
        t!(fs::create_dir_all(dir));
    }
    let mut cmd = rust_installer(build);
    cmd.arg("tarball")
       .arg("--input").arg(&plain_name)
       .arg("--output").arg(&tarball)
       .arg("--work-dir=.")
       .current_dir(tmpdir(build));
    build.run(&mut cmd);
}

fn install(src: &Path, dstdir: &Path, perms: u32) {
    let dst = dstdir.join(src.file_name().unwrap());
    t!(fs::create_dir_all(dstdir));
    t!(fs::copy(src, &dst));
    chmod(&dst, perms);
}

#[cfg(unix)]
fn chmod(path: &Path, perms: u32) {
    use std::os::unix::fs::*;
    t!(fs::set_permissions(path, fs::Permissions::from_mode(perms)));
}
#[cfg(windows)]
fn chmod(_path: &Path, _perms: u32) {}

// We have to run a few shell scripts, which choke quite a bit on both `\`
// characters and on `C:\` paths, so normalize both of them away.
pub fn sanitize_sh(path: &Path) -> String {
    let path = path.to_str().unwrap().replace("\\", "/");
    return change_drive(&path).unwrap_or(path);

    fn change_drive(s: &str) -> Option<String> {
        let mut ch = s.chars();
        let drive = ch.next().unwrap_or('C');
        if ch.next() != Some(':') {
            return None
        }
        if ch.next() != Some('/') {
            return None
        }
        Some(format!("/{}/{}", drive, &s[drive.len_utf8() + 2..]))
    }
}

fn write_file(path: &Path, data: &[u8]) {
    let mut vf = t!(fs::File::create(path));
    t!(vf.write_all(data));
}

pub fn cargo(build: &Build, stage: u32, target: &str) {
    println!("Dist cargo stage{} ({})", stage, target);
    let compiler = Compiler::new(stage, &build.config.build);

    let src = build.src.join("src/tools/cargo");
    let etc = src.join("src/etc");
    let release_num = build.release_num("cargo");
    let name = pkgname(build, "cargo");
    let version = build.cargo_info.version(build, &release_num);

    let tmp = tmpdir(build);
    let image = tmp.join("cargo-image");
    drop(fs::remove_dir_all(&image));
    t!(fs::create_dir_all(&image));

    // Prepare the image directory
    t!(fs::create_dir_all(image.join("share/zsh/site-functions")));
    t!(fs::create_dir_all(image.join("etc/bash_completion.d")));
    let cargo = build.cargo_out(&compiler, Mode::Tool, target)
                     .join(exe("cargo", target));
    install(&cargo, &image.join("bin"), 0o755);
    for man in t!(etc.join("man").read_dir()) {
        let man = t!(man);
        install(&man.path(), &image.join("share/man/man1"), 0o644);
    }
    install(&etc.join("_cargo"), &image.join("share/zsh/site-functions"), 0o644);
    copy(&etc.join("cargo.bashcomp.sh"),
         &image.join("etc/bash_completion.d/cargo"));
    let doc = image.join("share/doc/cargo");
    install(&src.join("README.md"), &doc, 0o644);
    install(&src.join("LICENSE-MIT"), &doc, 0o644);
    install(&src.join("LICENSE-APACHE"), &doc, 0o644);
    install(&src.join("LICENSE-THIRD-PARTY"), &doc, 0o644);

    // Prepare the overlay
    let overlay = tmp.join("cargo-overlay");
    drop(fs::remove_dir_all(&overlay));
    t!(fs::create_dir_all(&overlay));
    install(&src.join("README.md"), &overlay, 0o644);
    install(&src.join("LICENSE-MIT"), &overlay, 0o644);
    install(&src.join("LICENSE-APACHE"), &overlay, 0o644);
    install(&src.join("LICENSE-THIRD-PARTY"), &overlay, 0o644);
    t!(t!(File::create(overlay.join("version"))).write_all(version.as_bytes()));

    // Generate the installer tarball
    let mut cmd = rust_installer(build);
    cmd.arg("generate")
       .arg("--product-name=Rust")
       .arg("--rel-manifest-dir=rustlib")
       .arg("--success-message=Rust-is-ready-to-roll.")
       .arg("--image-dir").arg(&image)
       .arg("--work-dir").arg(&tmpdir(build))
       .arg("--output-dir").arg(&distdir(build))
       .arg("--non-installed-overlay").arg(&overlay)
       .arg(format!("--package-name={}-{}", name, target))
       .arg("--component-name=cargo")
       .arg("--legacy-manifest-dirs=rustlib,cargo");
    build.run(&mut cmd);
}

pub fn rls(build: &Build, stage: u32, target: &str) {
    assert!(build.config.extended);
    println!("Dist RLS stage{} ({})", stage, target);
    let compiler = Compiler::new(stage, &build.config.build);

    let src = build.src.join("src/tools/rls");
    let release_num = build.release_num("rls");
    let name = pkgname(build, "rls");
    let version = build.rls_info.version(build, &release_num);

    let tmp = tmpdir(build);
    let image = tmp.join("rls-image");
    drop(fs::remove_dir_all(&image));
    t!(fs::create_dir_all(&image));

    // Prepare the image directory
    let rls = build.cargo_out(&compiler, Mode::Tool, target)
                     .join(exe("rls", target));
    install(&rls, &image.join("bin"), 0o755);
    let doc = image.join("share/doc/rls");
    install(&src.join("README.md"), &doc, 0o644);
    install(&src.join("LICENSE-MIT"), &doc, 0o644);
    install(&src.join("LICENSE-APACHE"), &doc, 0o644);

    // Prepare the overlay
    let overlay = tmp.join("rls-overlay");
    drop(fs::remove_dir_all(&overlay));
    t!(fs::create_dir_all(&overlay));
    install(&src.join("README.md"), &overlay, 0o644);
    install(&src.join("LICENSE-MIT"), &overlay, 0o644);
    install(&src.join("LICENSE-APACHE"), &overlay, 0o644);
    t!(t!(File::create(overlay.join("version"))).write_all(version.as_bytes()));

    // Generate the installer tarball
    let mut cmd = rust_installer(build);
    cmd.arg("generate")
       .arg("--product-name=Rust")
       .arg("--rel-manifest-dir=rustlib")
       .arg("--success-message=RLS-ready-to-serve.")
       .arg("--image-dir").arg(&image)
       .arg("--work-dir").arg(&tmpdir(build))
       .arg("--output-dir").arg(&distdir(build))
       .arg("--non-installed-overlay").arg(&overlay)
       .arg(format!("--package-name={}-{}", name, target))
       .arg("--component-name=rls")
       .arg("--legacy-manifest-dirs=rustlib,cargo");
    build.run(&mut cmd);
}

/// Creates a combined installer for the specified target in the provided stage.
pub fn extended(build: &Build, stage: u32, target: &str) {
    println!("Dist extended stage{} ({})", stage, target);

    let dist = distdir(build);
    let rustc_installer = dist.join(format!("{}-{}.tar.gz",
                                            pkgname(build, "rustc"),
                                            target));
    let cargo_installer = dist.join(format!("{}-{}.tar.gz",
                                            pkgname(build, "cargo"),
                                            target));
    let rls_installer = dist.join(format!("{}-{}.tar.gz",
                                          pkgname(build, "rls"),
                                          target));
    let analysis_installer = dist.join(format!("{}-{}.tar.gz",
                                               pkgname(build, "rust-analysis"),
                                               target));
    let docs_installer = dist.join(format!("{}-{}.tar.gz",
                                           pkgname(build, "rust-docs"),
                                           target));
    let mingw_installer = dist.join(format!("{}-{}.tar.gz",
                                            pkgname(build, "rust-mingw"),
                                            target));
    let std_installer = dist.join(format!("{}-{}.tar.gz",
                                          pkgname(build, "rust-std"),
                                          target));

    let tmp = tmpdir(build);
    let overlay = tmp.join("extended-overlay");
    let etc = build.src.join("src/etc/installer");
    let work = tmp.join("work");

    let _ = fs::remove_dir_all(&overlay);
    install(&build.src.join("COPYRIGHT"), &overlay, 0o644);
    install(&build.src.join("LICENSE-APACHE"), &overlay, 0o644);
    install(&build.src.join("LICENSE-MIT"), &overlay, 0o644);
    let version = build.rust_version();
    t!(t!(File::create(overlay.join("version"))).write_all(version.as_bytes()));
    install(&etc.join("README.md"), &overlay, 0o644);

    // When rust-std package split from rustc, we needed to ensure that during
    // upgrades rustc was upgraded before rust-std. To avoid rustc clobbering
    // the std files during uninstall. To do this ensure that rustc comes
    // before rust-std in the list below.
    let mut tarballs = vec![rustc_installer, cargo_installer, rls_installer,
                            analysis_installer, docs_installer, std_installer];
    if target.contains("pc-windows-gnu") {
        tarballs.push(mingw_installer);
    }
    let mut input_tarballs = tarballs[0].as_os_str().to_owned();
    for tarball in &tarballs[1..] {
        input_tarballs.push(",");
        input_tarballs.push(tarball);
    }

    let mut cmd = rust_installer(build);
    cmd.arg("combine")
       .arg("--product-name=Rust")
       .arg("--rel-manifest-dir=rustlib")
       .arg("--success-message=Rust-is-ready-to-roll.")
       .arg("--work-dir").arg(&work)
       .arg("--output-dir").arg(&distdir(build))
       .arg(format!("--package-name={}-{}", pkgname(build, "rust"), target))
       .arg("--legacy-manifest-dirs=rustlib,cargo")
       .arg("--input-tarballs").arg(input_tarballs)
       .arg("--non-installed-overlay").arg(&overlay);
    build.run(&mut cmd);

    let mut license = String::new();
    t!(t!(File::open(build.src.join("COPYRIGHT"))).read_to_string(&mut license));
    license.push_str("\n");
    t!(t!(File::open(build.src.join("LICENSE-APACHE"))).read_to_string(&mut license));
    license.push_str("\n");
    t!(t!(File::open(build.src.join("LICENSE-MIT"))).read_to_string(&mut license));

    let rtf = r"{\rtf1\ansi\deff0{\fonttbl{\f0\fnil\fcharset0 Arial;}}\nowwrap\fs18";
    let mut rtf = rtf.to_string();
    rtf.push_str("\n");
    for line in license.lines() {
        rtf.push_str(line);
        rtf.push_str("\\line ");
    }
    rtf.push_str("}");

    if target.contains("apple-darwin") {
        let pkg = tmp.join("pkg");
        let _ = fs::remove_dir_all(&pkg);
        t!(fs::create_dir_all(pkg.join("rustc")));
        t!(fs::create_dir_all(pkg.join("cargo")));
        t!(fs::create_dir_all(pkg.join("rust-docs")));
        t!(fs::create_dir_all(pkg.join("rust-std")));

        cp_r(&work.join(&format!("{}-{}", pkgname(build, "rustc"), target)),
             &pkg.join("rustc"));
        cp_r(&work.join(&format!("{}-{}", pkgname(build, "cargo"), target)),
             &pkg.join("cargo"));
        cp_r(&work.join(&format!("{}-{}", pkgname(build, "rust-docs"), target)),
             &pkg.join("rust-docs"));
        cp_r(&work.join(&format!("{}-{}", pkgname(build, "rust-std"), target)),
             &pkg.join("rust-std"));

        install(&etc.join("pkg/postinstall"), &pkg.join("rustc"), 0o755);
        install(&etc.join("pkg/postinstall"), &pkg.join("cargo"), 0o755);
        install(&etc.join("pkg/postinstall"), &pkg.join("rust-docs"), 0o755);
        install(&etc.join("pkg/postinstall"), &pkg.join("rust-std"), 0o755);

        let pkgbuild = |component: &str| {
            let mut cmd = Command::new("pkgbuild");
            cmd.arg("--identifier").arg(format!("org.rust-lang.{}", component))
               .arg("--scripts").arg(pkg.join(component))
               .arg("--nopayload")
               .arg(pkg.join(component).with_extension("pkg"));
            build.run(&mut cmd);
        };
        pkgbuild("rustc");
        pkgbuild("cargo");
        pkgbuild("rust-docs");
        pkgbuild("rust-std");

        // create an 'uninstall' package
        install(&etc.join("pkg/postinstall"), &pkg.join("uninstall"), 0o755);
        pkgbuild("uninstall");

        t!(fs::create_dir_all(pkg.join("res")));
        t!(t!(File::create(pkg.join("res/LICENSE.txt"))).write_all(license.as_bytes()));
        install(&etc.join("gfx/rust-logo.png"), &pkg.join("res"), 0o644);
        let mut cmd = Command::new("productbuild");
        cmd.arg("--distribution").arg(etc.join("pkg/Distribution.xml"))
           .arg("--resources").arg(pkg.join("res"))
           .arg(distdir(build).join(format!("{}-{}.pkg",
                                             pkgname(build, "rust"),
                                             target)))
           .arg("--package-path").arg(&pkg);
        build.run(&mut cmd);
    }

    if target.contains("windows") {
        let exe = tmp.join("exe");
        let _ = fs::remove_dir_all(&exe);
        t!(fs::create_dir_all(exe.join("rustc")));
        t!(fs::create_dir_all(exe.join("cargo")));
        t!(fs::create_dir_all(exe.join("rust-docs")));
        t!(fs::create_dir_all(exe.join("rust-std")));
        cp_r(&work.join(&format!("{}-{}", pkgname(build, "rustc"), target))
                  .join("rustc"),
             &exe.join("rustc"));
        cp_r(&work.join(&format!("{}-{}", pkgname(build, "cargo"), target))
                  .join("cargo"),
             &exe.join("cargo"));
        cp_r(&work.join(&format!("{}-{}", pkgname(build, "rust-docs"), target))
                  .join("rust-docs"),
             &exe.join("rust-docs"));
        cp_r(&work.join(&format!("{}-{}", pkgname(build, "rust-std"), target))
                  .join(format!("rust-std-{}", target)),
             &exe.join("rust-std"));

        t!(fs::remove_file(exe.join("rustc/manifest.in")));
        t!(fs::remove_file(exe.join("cargo/manifest.in")));
        t!(fs::remove_file(exe.join("rust-docs/manifest.in")));
        t!(fs::remove_file(exe.join("rust-std/manifest.in")));

        if target.contains("windows-gnu") {
            t!(fs::create_dir_all(exe.join("rust-mingw")));
            cp_r(&work.join(&format!("{}-{}", pkgname(build, "rust-mingw"), target))
                      .join("rust-mingw"),
                 &exe.join("rust-mingw"));
            t!(fs::remove_file(exe.join("rust-mingw/manifest.in")));
        }

        install(&etc.join("exe/rust.iss"), &exe, 0o644);
        install(&etc.join("exe/modpath.iss"), &exe, 0o644);
        install(&etc.join("exe/upgrade.iss"), &exe, 0o644);
        install(&etc.join("gfx/rust-logo.ico"), &exe, 0o644);
        t!(t!(File::create(exe.join("LICENSE.txt"))).write_all(license.as_bytes()));

        // Generate exe installer
        let mut cmd = Command::new("iscc");
        cmd.arg("rust.iss")
           .current_dir(&exe);
        if target.contains("windows-gnu") {
            cmd.arg("/dMINGW");
        }
        add_env(build, &mut cmd, target);
        build.run(&mut cmd);
        install(&exe.join(format!("{}-{}.exe", pkgname(build, "rust"), target)),
                &distdir(build),
                0o755);

        // Generate msi installer
        let wix = PathBuf::from(env::var_os("WIX").unwrap());
        let heat = wix.join("bin/heat.exe");
        let candle = wix.join("bin/candle.exe");
        let light = wix.join("bin/light.exe");

        let heat_flags = ["-nologo", "-gg", "-sfrag", "-srd", "-sreg"];
        build.run(Command::new(&heat)
                        .current_dir(&exe)
                        .arg("dir")
                        .arg("rustc")
                        .args(&heat_flags)
                        .arg("-cg").arg("RustcGroup")
                        .arg("-dr").arg("Rustc")
                        .arg("-var").arg("var.RustcDir")
                        .arg("-out").arg(exe.join("RustcGroup.wxs")));
        build.run(Command::new(&heat)
                        .current_dir(&exe)
                        .arg("dir")
                        .arg("rust-docs")
                        .args(&heat_flags)
                        .arg("-cg").arg("DocsGroup")
                        .arg("-dr").arg("Docs")
                        .arg("-var").arg("var.DocsDir")
                        .arg("-out").arg(exe.join("DocsGroup.wxs"))
                        .arg("-t").arg(etc.join("msi/squash-components.xsl")));
        build.run(Command::new(&heat)
                        .current_dir(&exe)
                        .arg("dir")
                        .arg("cargo")
                        .args(&heat_flags)
                        .arg("-cg").arg("CargoGroup")
                        .arg("-dr").arg("Cargo")
                        .arg("-var").arg("var.CargoDir")
                        .arg("-out").arg(exe.join("CargoGroup.wxs"))
                        .arg("-t").arg(etc.join("msi/remove-duplicates.xsl")));
        build.run(Command::new(&heat)
                        .current_dir(&exe)
                        .arg("dir")
                        .arg("rust-std")
                        .args(&heat_flags)
                        .arg("-cg").arg("StdGroup")
                        .arg("-dr").arg("Std")
                        .arg("-var").arg("var.StdDir")
                        .arg("-out").arg(exe.join("StdGroup.wxs")));
        if target.contains("windows-gnu") {
            build.run(Command::new(&heat)
                            .current_dir(&exe)
                            .arg("dir")
                            .arg("rust-mingw")
                            .args(&heat_flags)
                            .arg("-cg").arg("GccGroup")
                            .arg("-dr").arg("Gcc")
                            .arg("-var").arg("var.GccDir")
                            .arg("-out").arg(exe.join("GccGroup.wxs")));
        }

        let candle = |input: &Path| {
            let output = exe.join(input.file_stem().unwrap())
                            .with_extension("wixobj");
            let arch = if target.contains("x86_64") {"x64"} else {"x86"};
            let mut cmd = Command::new(&candle);
            cmd.current_dir(&exe)
               .arg("-nologo")
               .arg("-dRustcDir=rustc")
               .arg("-dDocsDir=rust-docs")
               .arg("-dCargoDir=cargo")
               .arg("-dStdDir=rust-std")
               .arg("-arch").arg(&arch)
               .arg("-out").arg(&output)
               .arg(&input);
            add_env(build, &mut cmd, target);

            if target.contains("windows-gnu") {
               cmd.arg("-dGccDir=rust-mingw");
            }
            build.run(&mut cmd);
        };
        candle(&etc.join("msi/rust.wxs"));
        candle(&etc.join("msi/ui.wxs"));
        candle(&etc.join("msi/rustwelcomedlg.wxs"));
        candle("RustcGroup.wxs".as_ref());
        candle("DocsGroup.wxs".as_ref());
        candle("CargoGroup.wxs".as_ref());
        candle("StdGroup.wxs".as_ref());

        if target.contains("windows-gnu") {
            candle("GccGroup.wxs".as_ref());
        }

        t!(t!(File::create(exe.join("LICENSE.rtf"))).write_all(rtf.as_bytes()));
        install(&etc.join("gfx/banner.bmp"), &exe, 0o644);
        install(&etc.join("gfx/dialogbg.bmp"), &exe, 0o644);

        let filename = format!("{}-{}.msi", pkgname(build, "rust"), target);
        let mut cmd = Command::new(&light);
        cmd.arg("-nologo")
           .arg("-ext").arg("WixUIExtension")
           .arg("-ext").arg("WixUtilExtension")
           .arg("-out").arg(exe.join(&filename))
           .arg("rust.wixobj")
           .arg("ui.wixobj")
           .arg("rustwelcomedlg.wixobj")
           .arg("RustcGroup.wixobj")
           .arg("DocsGroup.wixobj")
           .arg("CargoGroup.wixobj")
           .arg("StdGroup.wixobj")
           .current_dir(&exe);

        if target.contains("windows-gnu") {
           cmd.arg("GccGroup.wixobj");
        }
        // ICE57 wrongly complains about the shortcuts
        cmd.arg("-sice:ICE57");

        build.run(&mut cmd);

        t!(fs::rename(exe.join(&filename), distdir(build).join(&filename)));
    }
}

fn add_env(build: &Build, cmd: &mut Command, target: &str) {
    let mut parts = channel::CFG_RELEASE_NUM.split('.');
    cmd.env("CFG_RELEASE_INFO", build.rust_version())
       .env("CFG_RELEASE_NUM", channel::CFG_RELEASE_NUM)
       .env("CFG_RELEASE", build.rust_release())
       .env("CFG_PRERELEASE_VERSION", channel::CFG_PRERELEASE_VERSION)
       .env("CFG_VER_MAJOR", parts.next().unwrap())
       .env("CFG_VER_MINOR", parts.next().unwrap())
       .env("CFG_VER_PATCH", parts.next().unwrap())
       .env("CFG_VER_BUILD", "0") // just needed to build
       .env("CFG_PACKAGE_VERS", build.rust_package_vers())
       .env("CFG_PACKAGE_NAME", pkgname(build, "rust"))
       .env("CFG_BUILD", target)
       .env("CFG_CHANNEL", &build.config.channel);

    if target.contains("windows-gnu") {
       cmd.env("CFG_MINGW", "1")
          .env("CFG_ABI", "GNU");
    } else {
       cmd.env("CFG_MINGW", "0")
          .env("CFG_ABI", "MSVC");
    }

    if target.contains("x86_64") {
       cmd.env("CFG_PLATFORM", "x64");
    } else {
       cmd.env("CFG_PLATFORM", "x86");
    }
}

pub fn hash_and_sign(build: &Build) {
    let compiler = Compiler::new(0, &build.config.build);
    let mut cmd = build.tool_cmd(&compiler, "build-manifest");
    let sign = build.config.dist_sign_folder.as_ref().unwrap_or_else(|| {
        panic!("\n\nfailed to specify `dist.sign-folder` in `config.toml`\n\n")
    });
    let addr = build.config.dist_upload_addr.as_ref().unwrap_or_else(|| {
        panic!("\n\nfailed to specify `dist.upload-addr` in `config.toml`\n\n")
    });
    let file = build.config.dist_gpg_password_file.as_ref().unwrap_or_else(|| {
        panic!("\n\nfailed to specify `dist.gpg-password-file` in `config.toml`\n\n")
    });
    let mut pass = String::new();
    t!(t!(File::open(&file)).read_to_string(&mut pass));

    let today = output(Command::new("date").arg("+%Y-%m-%d"));

    cmd.arg(sign);
    cmd.arg(distdir(build));
    cmd.arg(today.trim());
    cmd.arg(build.rust_package_vers());
    cmd.arg(build.package_vers(&build.release_num("cargo")));
    cmd.arg(build.package_vers(&build.release_num("rls")));
    cmd.arg(addr);

    t!(fs::create_dir_all(distdir(build)));

    let mut child = t!(cmd.stdin(Stdio::piped()).spawn());
    t!(child.stdin.take().unwrap().write_all(pass.as_bytes()));
    let status = t!(child.wait());
    assert!(status.success());
}
