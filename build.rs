use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Only embed admin manifest for release builds
    // Debug/test builds should run without elevation for easier development
    let profile = env::var("PROFILE").unwrap_or_default();
    if profile == "release" {
        println!("cargo:rerun-if-changed=app.manifest");
        let mut res = winres::WindowsResource::new();
        res.set_manifest(include_str!("app.manifest"));
        res.compile().expect("failed to add app manifest");
    }

    println!("cargo:rerun-if-changed=wrapper/Program.cs");
    println!("cargo:rerun-if-changed=wrapper/wrapper.csproj");
    let status = Command::new("dotnet")
        .arg("publish")
        .arg("--configuration")
        .arg("Release")
        .current_dir("wrapper")
        .status()
        .expect("failed to build wrapper");
    if !status.success() {
        panic!("failed to build wrapper: {}", status);
    }

    // Copy wrapper to target directory
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let target_dir = Path::new("target").join(&profile);
    let wrapper_src = Path::new("wrapper/bin/Release/net8.0-windows/win-x64/publish/wrapper.exe");
    let wrapper_dst = target_dir.join("thelio-io_wrapper.exe");

    if wrapper_src.exists() {
        fs::create_dir_all(&target_dir).expect("failed to create target directory");
        fs::copy(&wrapper_src, &wrapper_dst).expect("failed to copy wrapper to target directory");
    }
}
