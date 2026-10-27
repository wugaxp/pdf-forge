//! `cargo xtask version [check | set X.Y.Z[-pre]]`: the workspace version, the single source of truth.
//!
//! The version lives in `[workspace.package] version` in the root `Cargo.toml`; every crate
//! inherits it with `version.workspace = true`, and the packaging scripts read it from here.
//! The `version` of each workspace path dependency in `[workspace.dependencies]` is pinned to it
//! and must stay equal (`check`, a step of `cargo xtask ci` and a unit test).

use std::path::Path;

/// The `version = "…"` value inside `[workspace.package]`.
pub fn read(manifest: &str) -> Result<String, String> {
    let (_, line) = find_line(manifest)?;
    let v = line.split_once('=').map(|(_, v)| v.trim().trim_matches('"').to_string()).unwrap_or_default();
    if v.is_empty() {
        return Err("empty [workspace.package] version".into());
    }
    Ok(v)
}

/// `manifest` with the workspace version replaced, together with the matching `version` of every
/// path dependency in `[workspace.dependencies]` (the workspace's own crates, which must agree
/// with it or `cargo update` refuses them); everything else is byte-for-byte unchanged.
pub fn replace(manifest: &str, new: &str) -> Result<String, String> {
    validate(new)?;
    let old = read(manifest)?;
    let (idx, _) = find_line(manifest)?;
    let (old_pin, new_pin) = (format!("version = \"{old}\""), format!("version = \"{new}\""));
    let mut out = String::with_capacity(manifest.len() + 8);
    let mut in_deps = false;
    for (i, line) in manifest.split_inclusive('\n').enumerate() {
        let t = line.trim();
        if t.starts_with('[') {
            in_deps = t == "[workspace.dependencies]";
        }
        if i == idx {
            let eol = if line.ends_with("\r\n") {
                "\r\n"
            } else if line.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            out.push_str(&format!("version = \"{new}\"{eol}"));
        } else if in_deps && line.contains("path = \"") && line.contains(&old_pin) {
            out.push_str(&line.replace(&old_pin, &new_pin));
        } else {
            out.push_str(line);
        }
    }
    Ok(out)
}

/// The `version = "…"` pin of every path dependency in `[workspace.dependencies]`, in file order:
/// (line number, dependency name, pinned version). These pins are the workspace's own crates and
/// must agree with the workspace version, or `cargo update` refuses them.
pub fn path_dep_pins(manifest: &str) -> Vec<(usize, String, String)> {
    let mut pins = Vec::new();
    let mut in_deps = false;
    for (i, line) in manifest.lines().enumerate() {
        let t = line.trim();
        if t.starts_with('[') {
            in_deps = t == "[workspace.dependencies]";
            continue;
        }
        if !in_deps || !line.contains("path = \"") {
            continue;
        }
        let Some((name, rest)) = t.split_once(" = {") else { continue };
        let Some((_, version)) = rest.split_once("version = \"") else { continue };
        let Some((version, _)) = version.split_once('"') else { continue };
        pins.push((i + 1, name.trim().to_string(), version.to_string()));
    }
    pins
}

/// The workspace version, once every workspace path-dependency pin agrees with it. The error
/// names each disagreeing pin and its line, so the failing manifest edit is obvious.
pub fn check(manifest: &str) -> Result<String, String> {
    let version = read(manifest)?;
    let disagreements: Vec<String> = path_dep_pins(manifest)
        .into_iter()
        .filter(|(_, _, pin)| *pin != version)
        .map(|(line, name, pin)| format!("line {line}: {name} is pinned to {pin}"))
        .collect();
    if disagreements.is_empty() {
        Ok(version)
    } else {
        Err(format!("workspace path-dependency pins disagree with version {version}: {}", disagreements.join("; ")))
    }
}

/// `cargo xtask version check` and the `versions` step of `cargo xtask ci`: the root manifest's
/// path-dependency pins agree with `[workspace.package] version`.
pub fn verify(root: &Path) -> Result<(), String> {
    let path = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let version = check(&text)?;
    println!("version {version}: every workspace path-dependency pin agrees");
    Ok(())
}

/// Semver without build metadata: `MAJOR.MINOR.PATCH` plus an optional `-pre.release` tag.
pub fn validate(v: &str) -> Result<(), String> {
    let bad = || Err(format!("`{v}` is not a version like 1.2.3 or 1.2.3-rc.1"));
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let nums: Vec<&str> = core.split('.').collect();
    if nums.len() != 3 || nums.iter().any(|n| n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) || (n.len() > 1 && n.starts_with('0'))) {
        return bad();
    }
    if let Some(p) = pre
        && (p.is_empty() || p.split('.').any(|id| id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')))
    {
        return bad();
    }
    Ok(())
}

/// Index and text of the `version` line in `[workspace.package]`.
fn find_line(manifest: &str) -> Result<(usize, &str), String> {
    let mut in_section = false;
    for (i, line) in manifest.lines().enumerate() {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t == "[workspace.package]";
            continue;
        }
        if in_section
            && let Some(rest) = t.strip_prefix("version")
            && rest.trim_start().starts_with('=')
        {
            return Ok((i, line));
        }
    }
    Err("no `version = \"…\"` in [workspace.package] of the root Cargo.toml".into())
}

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let path = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    match args {
        [] => {
            println!("{}", read(&text)?);
            Ok(())
        }
        ["set", new] => {
            let old = read(&text)?;
            let updated = replace(&text, new)?;
            // Write atomically: a half-written root manifest breaks every build in the workspace.
            let tmp = root.join("target").join("Cargo.toml.xtask-version");
            std::fs::create_dir_all(root.join("target")).map_err(|e| format!("create target/: {e}"))?;
            std::fs::write(&tmp, updated).map_err(|e| format!("write {}: {e}", tmp.display()))?;
            std::fs::rename(&tmp, &path).map_err(|e| format!("replace {}: {e}", path.display()))?;
            // Refresh the workspace members' entries in Cargo.lock (no dependency upgrades).
            let cargo_update = |extra: &[&str]| -> Result<(), String> {
                let st = std::process::Command::new(env!("CARGO"))
                    .current_dir(root)
                    .args(["update", "--workspace"])
                    .args(extra)
                    .status()
                    .map_err(|e| format!("cargo update: {e}"))?;
                if st.success() { Ok(()) } else { Err(format!("cargo update --workspace {} failed", extra.join(" "))) }
            };
            if cargo_update(&["--offline"]).is_err() {
                cargo_update(&[])?;
            }
            println!("version: {old} -> {new}");
            Ok(())
        }
        ["check"] => verify(root),
        _ => Err("usage: cargo xtask version [check | set X.Y.Z[-pre]]".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "[workspace]\nmembers = [\"a\"]\n\n[workspace.package]\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace.dependencies]\nfoo = { version = \"1\" }\n";

    #[test]
    fn reads_the_workspace_version() {
        assert_eq!(read(MANIFEST).unwrap(), "0.1.0");
        assert!(read("[package]\nversion = \"1.0.0\"\n").is_err());
    }

    #[test]
    fn replaces_only_the_workspace_version() {
        let out = replace(MANIFEST, "1.2.3-rc.1").unwrap();
        assert_eq!(read(&out).unwrap(), "1.2.3-rc.1");
        assert_eq!(out, MANIFEST.replace("version = \"0.1.0\"", "version = \"1.2.3-rc.1\""));
        assert!(out.contains("foo = { version = \"1\" }"));
    }

    #[test]
    fn moves_the_workspace_crates_pins_with_it() {
        let manifest = "[workspace.package]\nversion = \"0.1.0\"\n\n[workspace.dependencies]\n\
                        own = { path = \"crates/own\", version = \"0.1.0\" }\n\
                        dep = { version = \"0.1.0\" }\n\
                        other = { path = \"crates/other\", version = \"0.2\" }\n";
        let out = replace(manifest, "0.2.0").unwrap();
        assert!(out.contains("own = { path = \"crates/own\", version = \"0.2.0\" }"), "{out}");
        assert!(out.contains("dep = { version = \"0.1.0\" }"), "a registry dependency keeps its version");
        assert!(out.contains("other = { path = \"crates/other\", version = \"0.2\" }"));
    }

    #[test]
    fn keeps_crlf_line_endings() {
        let crlf = MANIFEST.replace('\n', "\r\n");
        let out = replace(&crlf, "2.0.0").unwrap();
        assert_eq!(out, crlf.replace("\"0.1.0\"", "\"2.0.0\""));
    }

    #[test]
    fn validates_versions() {
        for ok in ["0.1.0", "1.20.300", "1.0.0-rc.1", "1.0.0-alpha", "1.0.0-x-y.2"] {
            assert!(validate(ok).is_ok(), "{ok}");
        }
        for bad in ["1.0", "1.0.0.0", "v1.0.0", "01.0.0", "1.0.0-", "1.0.0-a..b", "1.0.0+meta", "1.a.0", ""] {
            assert!(validate(bad).is_err(), "{bad}");
        }
        assert!(replace(MANIFEST, "nope").is_err());
    }

    #[test]
    fn the_real_manifest_has_a_valid_version() {
        let text = std::fs::read_to_string(crate::gates::root().join("Cargo.toml")).unwrap();
        validate(&read(&text).unwrap()).unwrap();
    }

    /// The gate `cargo xtask ci` runs: every workspace path-dependency pin agrees with
    /// `[workspace.package] version`, so `cargo update` accepts the workspace's own crates.
    #[test]
    fn the_workspace_crates_pins_agree_with_the_workspace_version() {
        let text = std::fs::read_to_string(crate::gates::root().join("Cargo.toml")).unwrap();
        let version = check(&text).unwrap();
        validate(&version).unwrap();
    }

    #[test]
    fn a_pin_that_disagrees_is_reported_with_its_line() {
        let manifest = "[workspace.package]\nversion = \"0.1.0\"\n\n[workspace.dependencies]\n\
                        own = { path = \"crates/own\", version = \"0.2.0\" }\n\
                        dep = { path = \"crates/dep\", version = \"0.1.0\" }\n\
                        reg = { version = \"0.1.0\" }\n";
        assert_eq!(path_dep_pins(manifest).len(), 2, "registry dependencies have no pin");
        let error = check(manifest).unwrap_err();
        assert!(error.contains("own is pinned to 0.2.0"), "{error}");
        // The agreeing pin is not reported ("dependency" in the generic sentence contains "dep",
        // so match the reported form, not the substring).
        assert!(!error.contains("dep is pinned"), "{error}");
    }

    /// The `rust-version = "1.NN"` line of a manifest, as the minor number.
    fn minor_of_rust_version(manifest: &str) -> u32 {
        let line = manifest.lines().find(|l| l.starts_with("rust-version = \"")).unwrap();
        line.split('"').nth(1).unwrap().split('.').nth(1).unwrap().parse().unwrap()
    }

    #[test]
    fn workspace_rust_version_covers_the_ui_crate() {
        let root = crate::gates::root();
        let workspace = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        let ui = std::fs::read_to_string(root.join("crates/ui-egui/Cargo.toml")).unwrap();
        assert!(minor_of_rust_version(&workspace) >= minor_of_rust_version(&ui));
    }
}
