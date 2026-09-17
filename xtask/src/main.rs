//! Cross-platform dev tasks (fetch-pdfium, fetch-onnxruntime, bench-corpus
//! now; fetch-models, gen-bindings land with the milestones that need them
//! — see SPEC.md §5.1).

mod bench_corpus;

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

/// ONNX Runtime release pinned from
/// https://github.com/microsoft/onnxruntime/releases — the same upstream
/// version (`ms@1.28.0`) that `ort` 2.0.0-rc.13 (our pinned `ort` version)
/// itself bundles when its `download-binaries` feature is used. We don't
/// use that feature (it fetches from a third-party CDN at build time,
/// outside our own manifest-controlled downloads) — instead we vendor the
/// official Microsoft release here, the same way `fetch_pdfium` vendors
/// PDFium, and load it at runtime via `ort`'s `load-dynamic` feature. Each
/// archive's SHA-256 is the whole downloaded release asset's hash,
/// re-verified whenever `ONNXRUNTIME_VERSION` changes (see SPEC.md §0:
/// "Never invent ... checksums").
const ONNXRUNTIME_VERSION: &str = "1.28.0";

struct OnnxRuntimeTarget {
    /// Directory name under `vendor/onnxruntime/`.
    dir: &'static str,
    archive_name: &'static str,
    archive_sha256: &'static str,
    /// Path of the shared library inside the archive.
    member_path: &'static str,
    /// Destination filename under `vendor/onnxruntime/<dir>/lib/`.
    lib_filename: &'static str,
    is_zip: bool,
}

const ONNXRUNTIME_TARGETS: &[OnnxRuntimeTarget] = &[
    OnnxRuntimeTarget {
        dir: "win-x64",
        archive_name: "onnxruntime-win-x64-1.28.0.zip",
        archive_sha256: "abef733dacbe2f571547a7150b479b5cb9cc0df22f96c24983a42cadb1b4f8bc",
        member_path: "onnxruntime-win-x64-1.28.0/lib/onnxruntime.dll",
        lib_filename: "onnxruntime.dll",
        is_zip: true,
    },
    OnnxRuntimeTarget {
        dir: "linux-x64",
        archive_name: "onnxruntime-linux-x64-1.28.0.tgz",
        archive_sha256: "a3e1b79d7bb1bf09696ce675f49e4064e6c81f6202b8225624fff0e93f8d6407",
        member_path: "onnxruntime-linux-x64-1.28.0/lib/libonnxruntime.so.1.28.0",
        lib_filename: "libonnxruntime.so",
        is_zip: false,
    },
    OnnxRuntimeTarget {
        dir: "mac-arm64",
        archive_name: "onnxruntime-osx-arm64-1.28.0.tgz",
        archive_sha256: "1268b359718099bde2cedb55787f182a130067bc4f31e8c88478c445b850d3d8",
        member_path: "onnxruntime-osx-arm64-1.28.0/lib/libonnxruntime.1.28.0.dylib",
        lib_filename: "libonnxruntime.dylib",
        is_zip: false,
    },
    // No `mac-x64` entry: Microsoft's v1.28.0 release doesn't publish an
    // Intel-Mac CPU build any more. `fetch_onnxruntime` gives a clear error
    // on that target rather than silently skipping it.
];

fn fetch_onnxruntime() -> Result<()> {
    let target = host_target()?;
    let spec = ONNXRUNTIME_TARGETS.iter().find(|t| t.dir == target).with_context(|| {
        format!(
            "no pinned ONNX Runtime {ONNXRUNTIME_VERSION} binary for {target} (Microsoft's official release doesn't publish one for this target)"
        )
    })?;

    let dest_dir = workspace_root()?
        .join("vendor/onnxruntime")
        .join(spec.dir)
        .join("lib");
    let dest_path = dest_dir.join(spec.lib_filename);
    let marker = dest_dir.join(".sha256");
    if dest_path.exists() && marker.exists() && fs::read_to_string(&marker)? == spec.archive_sha256
    {
        println!(
            "onnxruntime ({}) already present and verified at {}",
            spec.dir,
            dest_path.display()
        );
        return Ok(());
    }

    let url = format!(
        "https://github.com/microsoft/onnxruntime/releases/download/v{ONNXRUNTIME_VERSION}/{}",
        spec.archive_name
    );
    println!("downloading {url}");
    let bytes = ureq::get(&url)
        .call()
        .with_context(|| format!("GET {url}"))?
        .body_mut()
        .with_config()
        .limit(300 * 1024 * 1024)
        .read_to_vec()
        .context("reading ONNX Runtime archive body")?;

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if actual != spec.archive_sha256 {
        bail!(
            "checksum mismatch for {}: expected {}, got {actual}",
            spec.archive_name,
            spec.archive_sha256
        );
    }

    fs::create_dir_all(&dest_dir)?;
    if spec.is_zip {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes))
            .with_context(|| format!("opening {} as a zip archive", spec.archive_name))?;
        let mut member = archive
            .by_name(spec.member_path)
            .with_context(|| format!("{} missing from {}", spec.member_path, spec.archive_name))?;
        let mut out = fs::File::create(&dest_path)?;
        std::io::copy(&mut member, &mut out).context("writing extracted library")?;
    } else {
        // GNU tar (what Windows' Git Bash and Linux/macOS all have) reads
        // gzip natively but not zip, so `.tgz` extraction goes through
        // `tar` while the `.zip` case above uses the `zip` crate directly.
        let archive_path = dest_dir.join(spec.archive_name);
        fs::write(&archive_path, &bytes)?;
        let output = std::process::Command::new("tar")
            .args(["-xzf", spec.archive_name, "-O", spec.member_path])
            .current_dir(&dest_dir)
            .output()
            .context("running `tar` to extract the ONNX Runtime library")?;
        if !output.status.success() {
            bail!(
                "tar exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        fs::write(&dest_path, &output.stdout)?;
        fs::remove_file(&archive_path)?;
    }

    let mut marker_file = fs::File::create(&marker)?;
    marker_file.write_all(spec.archive_sha256.as_bytes())?;

    println!(
        "onnxruntime ({}) verified and extracted to {}",
        spec.dir,
        dest_path.display()
    );
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("fetch-pdfium") => fetch_pdfium(),
        Some("fetch-onnxruntime") => fetch_onnxruntime(),
        Some("bench-corpus") => bench_corpus::bench_corpus(),
        Some(other) => bail!("unknown xtask command: {other}"),
        None => bail!("usage: cargo xtask <fetch-pdfium|fetch-onnxruntime|bench-corpus>"),
    }
}
