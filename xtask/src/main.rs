//! Cross-platform dev tasks (fetch-pdfium now; fetch-models, gen-bindings,
//! bench-corpus land with the milestones that need them — see SPEC.md §5.1).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// PDFium release pinned from https://github.com/bblanchon/pdfium-binaries.
/// Version and per-target SHA-256 checksums must be re-verified whenever
/// `PDFIUM_TAG` changes (see SPEC.md §0: "Never invent ... checksums").
const PDFIUM_TAG: &str = "chromium/8044";

struct PdfiumTarget {
    /// Directory name under `vendor/pdfium/`, also the asset's target suffix.
    dir: &'static str,
    sha256: &'static str,
}

const PDFIUM_TARGETS: &[PdfiumTarget] = &[
    PdfiumTarget {
        dir: "win-x64",
        sha256: "78a17d9a5f14467631c26a3ac8741b27a0471ecc05bd6a119b523598160a0537",
    },
    PdfiumTarget {
        dir: "mac-arm64",
        sha256: "61424884d4a7f153b808deba6437848e4400834ce30aaf95d3050da44df8f420",
    },
    PdfiumTarget {
        dir: "mac-x64",
        sha256: "a93d44238e05de20028446561b951d50988b849efbbe56fe40c0d376c05b45e8",
    },
    PdfiumTarget {
        dir: "linux-x64",
        sha256: "eb142f416aed3a72fc5a02dbd5884868a16cb99dc0cf53e6bdd64afbf67b05f4",
    },
];

fn host_target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("win-x64"),
        ("macos", "aarch64") => Ok("mac-arm64"),
        ("macos", "x86_64") => Ok("mac-x64"),
        ("linux", "x86_64") => Ok("linux-x64"),
        (os, arch) => bail!("no pinned PDFium binary for {os}/{arch}"),
    }
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    Ok(manifest_dir
        .parent()
        .context("xtask has no parent directory")?
        .to_path_buf())
}

fn fetch_pdfium() -> Result<()> {
    let target = host_target()?;
    let spec = PDFIUM_TARGETS
        .iter()
        .find(|t| t.dir == target)
        .context("target missing from PDFIUM_TARGETS")?;

    let dest_dir = workspace_root()?.join("vendor/pdfium").join(spec.dir);
    let marker = dest_dir.join(".sha256");
    if marker.exists() && fs::read_to_string(&marker)? == spec.sha256 {
        println!(
            "pdfium ({}) already present and verified at {}",
            spec.dir,
            dest_dir.display()
        );
        return Ok(());
    }

    let url = format!(
        "https://github.com/bblanchon/pdfium-binaries/releases/download/{PDFIUM_TAG}/pdfium-{}.tgz",
        spec.dir
    );
    println!("downloading {url}");
    let bytes = ureq::get(&url)
        .call()
        .with_context(|| format!("GET {url}"))?
        .body_mut()
        .with_config()
        .limit(200 * 1024 * 1024)
        .read_to_vec()
        .context("reading PDFium archive body")?;

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if actual != spec.sha256 {
        bail!(
            "checksum mismatch for pdfium-{}: expected {}, got {actual}",
            spec.dir,
            spec.sha256
        );
    }

    fs::create_dir_all(&dest_dir)?;
    let archive_path = dest_dir.join("pdfium.tgz");
    fs::write(&archive_path, &bytes)?;

    let status = std::process::Command::new("tar")
        .args(["xzf", "pdfium.tgz"])
        .current_dir(&dest_dir)
        .status()
        .context("running `tar` to extract the PDFium archive")?;
    if !status.success() {
        bail!("tar exited with {status}");
    }
    fs::remove_file(&archive_path)?;

    let mut marker_file = fs::File::create(&marker)?;
    marker_file.write_all(spec.sha256.as_bytes())?;

    println!(
        "pdfium ({}) verified and extracted to {}",
        spec.dir,
        dest_dir.display()
    );
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("fetch-pdfium") => fetch_pdfium(),
        Some(other) => bail!("unknown xtask command: {other}"),
        None => bail!("usage: cargo xtask <fetch-pdfium>"),
    }
}
