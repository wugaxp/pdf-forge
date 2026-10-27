//! `cargo xtask screenshots [name…]`: regenerate the reference screenshots in `docs/images/`.
//!
//! Every image is a headless render of the real app (`examples/shot.rs`) showing the showcase
//! PDF. That PDF is built only from the OFL fonts and generated artwork in ATTRIBUTION.toml, so
//! the screenshots are contributor-original and policy-compliant (AGENTS.md §1). Afterwards the
//! `docs/images/*.png` entries in ATTRIBUTION.toml are rewritten with fresh hashes for the shots
//! this run rendered, and ATTRIBUTION.md is regenerated. Entries of images this run did not
//! render keep their recorded provenance: the images inherited from upstream stay attributed to
//! upstream until a run here replaces them.
//!
//! The document is opened by a *relative* path so no local directory names (user names) can
//! appear in a screenshot.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// What the screenshots in `docs/images/` are for. The fork's README shows none of them; they are
/// kept as reference renders of the interface.
const USAGE: &str = "Reference screenshot kept in docs/images";

/// (file stem, title, shot arguments after the PDF path).
pub const SCENES: &[(&str, &str, &[&str])] = &[
    ("pdfcraft-viewer", "Viewer with comments", &["--panel", "comments", "--zoom", "72"]),
    ("pdfcraft-dark", "Dark theme", &["--theme", "dark", "--panel", "comments", "--page", "11", "--zoom", "80", "--notice", "off"]),
    ("pdfcraft-organize", "Organize pages", &["--organize", "on", "--panel", "none", "--select", "5,6,7"]),
    ("pdfcraft-find", "Find text", &["--find", "type", "--page", "4", "--zoom", "90", "--panel", "none", "--notice", "off"]),
    ("pdfcraft-bookmarks", "Bookmarks", &["--panel", "bookmarks", "--page", "7", "--zoom", "75", "--notice", "off"]),
    ("pdfcraft-forms", "Interactive form", &["--page", "13", "--fields", "on", "--zoom", "80", "--panel", "fields"]),
    ("pdfcraft-layers", "Layers", &["--panel", "layers", "--page", "12", "--zoom", "75", "--notice", "off"]),
    ("pdfcraft-scripts", "Scripts of the world", &["--page", "7", "--zoom", "125", "--left", "closed", "--panel", "none", "--notice", "off"]),
    ("pdfcraft-properties", "Document properties", &["--dialog", "properties", "--notice", "off"]),
    ("pdfcraft-palette", "Command palette", &["--palette", "page", "--notice", "off"]),
    ("pdfcraft-split", "Split dialog", &["--organize", "on", "--panel", "none", "--dialog", "split", "--notice", "off"]),
    ("pdfcraft-tools", "All tools", &["--tools", "expanded", "--home", "on"]),
    ("pdfcraft-twoup", "Two-up reading", &["--layout", "two-up", "--mode", "read", "--page", "4", "--theme", "dark", "--notice", "off"]),
];

const PDF: &str = "dist/demo/pdfcraft-showcase.pdf";

pub fn run(args: &[String]) -> Result<()> {
    let root = crate::assets::root();
    if !root.join(PDF).is_file() {
        println!("screenshots: building the showcase PDF first");
        crate::demo_pdf::run(&[])?;
    }
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "-p", "pdfcraft-ui-egui", "--example", "shot"])
        .current_dir(&root)
        .status()
        .context("building the shot example")?;
    if !status.success() {
        bail!("building the shot example failed");
    }
    let target = std::env::var("CARGO_TARGET_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| root.join("target"));
    let shot = target.join("release/examples").join(if cfg!(windows) { "shot.exe" } else { "shot" });
    std::fs::create_dir_all(root.join("docs/images"))?;
    let mut rendered = BTreeSet::new();
    for (name, _, scene) in SCENES {
        if !args.is_empty() && !args.iter().any(|a| a == name) {
            continue;
        }
        let out = format!("docs/images/{name}.png");
        let status = Command::new(&shot)
            .arg(&out)
            .arg(PDF)
            .args(["--size", "1440x900", "--scale", "2", "--width", "1600"])
            .args(*scene)
            // Published images show only the embedded, openly licensed fonts (AGENTS.md §1.2).
            .env("PDFCRAFT_SYSTEM_FONTS", "0")
            .current_dir(&root)
            .status()
            .with_context(|| format!("running shot for {name}"))?;
        if !status.success() {
            bail!("screenshot {name} failed");
        }
        rendered.insert((*name).to_string());
    }
    update_attribution(&root, &rendered)?;
    crate::assets::run(&["--write".to_string()])
}

/// Replace the `docs/images/` entries of ATTRIBUTION.toml with one per screenshot this run
/// rendered. Entries of images left untouched keep their text, so screenshots that came from
/// upstream stay attributed to upstream instead of claiming this repository.
fn update_attribution(root: &Path, rendered: &BTreeSet<String>) -> Result<()> {
    let path = root.join("ATTRIBUTION.toml");
    let text = std::fs::read_to_string(&path)?;
    // Split into the header and `[[…]]` blocks; drop the blocks this run re-renders.
    let mut parts: Vec<String> = Vec::new();
    for (i, block) in text.split("\n[[").enumerate() {
        let block = if i == 0 { block.to_string() } else { format!("[[{block}") };
        let rerendered =
            block.starts_with("[[asset]]") && rendered.iter().any(|stem| block.contains(format!("path = \"docs/images/{stem}.png\"").as_str()));
        if rerendered {
            continue;
        }
        parts.push(block);
    }
    let mut out = parts.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    for stem in rendered {
        let scene = SCENES.iter().find(|(n, ..)| *n == stem.as_str());
        let title = scene.map(|(_, t, _)| *t).unwrap_or("PdfCraft screenshot");
        // The home screen shows the fork's PDF Forge mark (docs/brand/pdf-forge-mark.png), which
        // this repository's licence covers; say so next to the rest of what the shot shows.
        let shows_home = scene.is_some_and(|(.., args)| args.windows(2).any(|w| w[0] == "--home" && w[1] == "on"));
        let contents = if shows_home {
            "Shows PdfCraft UI, Lucide icons, the showcase PDF built from the [[fetched]] OFL fonts, and the PDF Forge mark (docs/brand/pdf-forge-mark.png)"
        } else {
            "Shows only PdfCraft UI, Lucide icons and the showcase PDF built from the [[fetched]] OFL fonts"
        };
        let bytes = std::fs::read(root.join("docs/images").join(format!("{stem}.png")))?;
        out.push_str(&format!(
            "\n[[asset]]\npath = \"docs/images/{stem}.png\"\ntitle = \"PdfCraft screenshot: {title}\"\nauthor = \"PdfCraft contributors (generated by `cargo xtask screenshots`)\"\nsource = \"this repository\"\nlicence = \"MIT OR Apache-2.0\"\nlicence_file = \"LICENSE-MIT\"\nkind = \"image\"\nusage = \"{USAGE}. {contents}\"\nsha256 = \"{}\"\n",
            crate::assets::sha256_hex(&bytes)
        ));
    }
    std::fs::write(&path, out)?;
    Ok(())
}
