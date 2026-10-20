// Copyright Kani Contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! This file is a glorified shell script for constructing a Kani release bundle.
//! We use Rust here just to aid in making the "script" more robust.
//!
//! Run with `cargo run -p build-kani -- release` and this will produce
//! (e.g.) `kani-1.0-x86_64-unknown-linux-gnu.tar.gz`.

mod parser;
mod sysroot;

use crate::sysroot::{
    build_bin, build_lib, build_tools, kani_no_core_lib, kani_playback_lib, kani_sysroot_lib,
};
use anyhow::{Result, bail};
use clap::Parser;
use std::{ffi::OsString, path::Path, process::Command};

fn main() -> Result<()> {
    let args = parser::ArgParser::parse();

    match args.subcommand {
        parser::Commands::BuildDev(build_parser) => {
            let bin_folder = &build_bin(&build_parser.args)?;
            if !build_parser.skip_libs {
                build_lib(bin_folder)?;
            }
            Ok(())
        }
        parser::Commands::Bundle(bundle_parser) => {
            let version_string = bundle_parser.version;
            let kani_string = format!("kani-{version_string}");
            let bundle_name = format!("{kani_string}-{}.tar.gz", env!("TARGET"));
            let dir = Path::new(&kani_string);

            // Check everything is ready before we start copying files
            println!("-- Build release bundle {bundle_name}");
            prebundle(dir)?;

            std::fs::create_dir(dir)?;

            bundle_kani(dir)?;
            bundle_cbmc(dir)?;
            bundle_kissat(dir)?;

            create_release_bundle(dir, &bundle_name)?;

            std::fs::remove_dir_all(dir)?;

            println!("\nSuccessfully built release bundle: {bundle_name}");

            Ok(())
        }
    }
}

/// Ensures everything is good to go before we begin to build the release bundle.
/// Notably, builds Kani in release mode.
fn prebundle(dir: &Path) -> Result<()> {
    if !Path::new("kani-compiler").exists() {
        bail!("Run from project root directory. Couldn't find 'kani-compiler'.");
    }

    if dir.exists() {
        bail!(
            "Directory {} already exists. Previous failed run? Delete it first.",
            dir.to_string_lossy()
        );
    }

    if which::which("cbmc").is_err() {
        bail!("Couldn't find the 'cbmc' binary to include in the release bundle.");
    }

    build_tools(&["--release"])?;

    // Before we begin, ensure Kani is built successfully in release mode.
    // And that libraries have been built too.
    build_lib(&build_bin(&["--release"])?)
}

/// Copy Kani files into `dir`
fn bundle_kani(dir: &Path) -> Result<()> {
    let bin = dir.join("bin");
    std::fs::create_dir(&bin)?;

    // 1. Kani binaries
    let release = Path::new("./target/release");
    for name in ["kani-driver", "kani-compiler", "kani-cov"] {
        cp(&release.join(name), &bin)?;
    }

    // 2. Kani scripts
    let scripts = dir.join("scripts");
    std::fs::create_dir(scripts)?;

    // 3. Kani libraries
    let library = dir.join("library");
    std::fs::create_dir(&library)?;

    cp_dir(Path::new("./library/kani"), &library)?;
    cp_dir(Path::new("./library/kani_macros"), &library)?;
    cp_dir(Path::new("./library/std"), &library)?;

    // 4. Pre-compiled library files
    cp_dir(&kani_sysroot_lib(), dir)?;
    cp_dir(kani_playback_lib().parent().unwrap(), dir)?;
    cp_dir(kani_no_core_lib().parent().unwrap(), dir)?;

    // 5. Record the exact toolchain and rustc version we use
    std::fs::write(dir.join("rust-toolchain-version"), env!("RUSTUP_TOOLCHAIN"))?;
    std::fs::write(dir.join("rustc-version"), get_rustc_version()?)?;

    // 6. Include a licensing note
    cp(Path::new("tools/build-kani/license-notes.txt"), dir)?;

    Ok(())
}

/// Copy the CBMC binaries selected by the release environment's PATH into `dir`.
fn bundle_cbmc(dir: &Path) -> Result<()> {
    bundle_path_tools(dir, &["cbmc", "goto-instrument", "goto-cc", "goto-analyzer"])
}

/// Copy Kissat binary into `dir`.
fn bundle_kissat(dir: &Path) -> Result<()> {
    bundle_path_tools(dir, &["kissat"])
}

fn bundle_path_tools(dir: &Path, tools: &[&str]) -> Result<()> {
    let bin = dir.join("bin");
    tools.iter().try_for_each(|tool| cp(&which::which(*tool)?, &bin))
}

/// Create the release tarball from `./dir` named `bundle`.
/// This should include all files as `dir/<path>` in the tarball.
/// e.g. `kani-1.0/bin/kani-compiler` not just `bin/kani-compiler`.
fn create_release_bundle(dir: &Path, bundle: &str) -> Result<()> {
    Command::new("tar").args(["zcf", bundle]).arg(dir).run()
}

/// Helper trait to fallibly run commands
trait AutoRun {
    fn run(&mut self) -> Result<()>;
}

impl AutoRun for Command {
    fn run(&mut self) -> Result<()> {
        let status = self.status()?;
        if !status.success() {
            bail!("Failed command: {}", render_command(self).to_string_lossy());
        }
        Ok(())
    }
}

fn expect_dir(path: &Path) -> Result<()> {
    if !path.is_dir() {
        bail!("{} isn't a directory", path.to_string_lossy());
    }
    Ok(())
}

/// Copy a single file to a directory
fn cp(src: &Path, dst: &Path) -> Result<()> {
    expect_dir(dst)?;
    let dst = dst.join(src.file_name().unwrap());
    std::fs::copy(src, dst)?;
    Ok(())
}

/// Record version of rustc being used to build Kani
fn get_rustc_version() -> Result<String> {
    Ok(String::from_utf8(Command::new("rustc").arg("--version").output().unwrap().stdout)?)
}

/// Copy files from `src` to  `dst` that respect the given pattern.
pub fn cp_files<P>(src: &Path, dst: &Path, predicate: P) -> Result<()>
where
    P: FnMut(&Path) -> bool,
{
    expect_dir(src)?;
    expect_dir(dst)?;
    let mut filter = predicate;
    for item in std::fs::read_dir(src)? {
        let path = item?.path();
        if filter(&path) {
            cp(&path, dst)?;
        }
    }
    Ok(())
}

/// Invoke `cp -r`
fn cp_dir(src: &Path, dst: &Path) -> Result<()> {
    let mut cmd = OsString::from("cp -r ");
    cmd.push(src.as_os_str());
    cmd.push(" ");
    cmd.push(dst.as_os_str());

    Command::new("bash").arg("-c").arg(cmd).run()
}

/// Render a Command as a string, to log it
pub fn render_command(cmd: &Command) -> OsString {
    let mut str = OsString::new();

    for (k, v) in cmd.get_envs() {
        if let Some(v) = v {
            str.push(k);
            str.push("=\"");
            str.push(v);
            str.push("\" ");
        }
    }

    str.push(cmd.get_program());

    for a in cmd.get_args() {
        str.push(" ");
        if a.to_string_lossy().contains(' ') {
            str.push("\"");
            str.push(a);
            str.push("\"");
        } else {
            str.push(a);
        }
    }

    str
}
