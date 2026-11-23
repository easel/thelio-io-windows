#!/usr/bin/env python3

import argparse
import glob
import os
import re
import shutil
import subprocess
import urllib.request
from zipfile import ZipFile


def read_version():
    """Read package version from Cargo.toml."""
    with open("Cargo.toml") as f:
        for line in f:
            m = re.match(r'^version\s*=\s*"(.+)"', line)
            if m:
                return m.group(1)
    raise RuntimeError("Could not find version in Cargo.toml")


VERSION = read_version()

# Files that get packaged into the MSI (must be signed before WiX build)
EXES_TO_SIGN = [
    r"target\release\thelio-io.exe",
    r"target\release\thelio-io-cli.exe",
    r"wrapper\bin\Release\net8.0-windows\win-x64\publish\wrapper.exe",
]

MSI_OUTPUT = rf"wix\thelio-io-{VERSION}-x86_64.msi"

# CodeSignTool setup
SIGN_DIR = r"target\sign"
TOOL_URL = "https://www.ssl.com/download/29773/"
TOOL_ZIP = os.path.join(SIGN_DIR, "CodeSignTool.zip")
TOOL_DIR = os.path.join(SIGN_DIR, "CodeSignTool")


def ensure_sign_tool():
    """Download and extract CodeSignTool if not already present."""
    os.makedirs(SIGN_DIR, exist_ok=True)

    if not os.path.isfile(TOOL_ZIP):
        partial = TOOL_ZIP + ".partial"
        if os.path.isfile(partial):
            os.remove(partial)
        print(f"Downloading CodeSignTool...")
        urllib.request.urlretrieve(TOOL_URL, partial)
        os.rename(partial, TOOL_ZIP)

    if not os.path.isdir(TOOL_DIR):
        partial = TOOL_DIR + ".partial"
        if os.path.isdir(partial):
            shutil.rmtree(partial)
        os.mkdir(partial)
        with ZipFile(TOOL_ZIP, "r") as zf:
            zf.extractall(partial)
        os.rename(partial, TOOL_DIR)


def find_tool_cwd():
    """Find the versioned CodeSignTool directory inside the extracted archive."""
    matches = glob.glob(os.path.join(TOOL_DIR, "CodeSignTool-v*-windows"))
    if not matches:
        raise FileNotFoundError(f"No CodeSignTool-v*-windows directory found in {TOOL_DIR}")
    return matches[0]


def sign_file(file_path, tool_cwd):
    """Sign a single file using ssl.com CodeSignTool."""
    abs_path = os.path.abspath(file_path)
    output_dir = os.path.dirname(abs_path)

    print(f"  Signing {file_path} ...")
    subprocess.check_call([
        "cmd", "/c", "CodeSignTool.bat",
        "sign",
        "-credential_id=" + os.environ["SSL_COM_CREDENTIAL_ID"],
        "-username=" + os.environ["SSL_COM_USERNAME"],
        "-password=" + os.environ["SSL_COM_PASSWORD"],
        "-totp_secret=" + os.environ["SSL_COM_TOTP_SECRET"],
        "-program_name=System76 Thelio Io",
        f"-input_file_path={abs_path}",
        f"-output_dir_path={output_dir}",
    ], cwd=tool_cwd)


# Handle commandline arguments
parser = argparse.ArgumentParser(description="Build and optionally sign thelio-io MSI")
parser.add_argument('--sign', action='store_true', help="Sign all executables and the MSI")
args = parser.parse_args()

# Step 1: Build all binaries
# cargo build --release triggers build.rs which also runs dotnet publish for the wrapper
print("=== Step 1: Building Rust binaries and .NET wrapper ===")
subprocess.check_call(["cargo", "build", "--release"])

# Step 2: Sign all EXEs before packaging
if args.sign:
    print("\n=== Step 2: Signing executables ===")
    ensure_sign_tool()
    tool_cwd = find_tool_cwd()
    for exe in EXES_TO_SIGN:
        sign_file(exe, tool_cwd)

# Step 3: Build MSI (packages the already-signed binaries)
print("\n=== Step 3: Building MSI ===")
subprocess.check_call([
    "wix", "build",
    r"wix\main.wxs",
    "-ext", "WixToolset.Util.wixext",
    "-ext", "WixToolset.UI.wixext",
    "-d", f"Version={VERSION}",
    "-d", "Profile=release",
    "-arch", "x64",
    "-o", MSI_OUTPUT,
])

# Step 4: Sign the MSI
if args.sign:
    print("\n=== Step 4: Signing MSI ===")
    tool_cwd = find_tool_cwd()
    sign_file(MSI_OUTPUT, tool_cwd)

print(f"\n=== Done! Output: {MSI_OUTPUT} ===")
