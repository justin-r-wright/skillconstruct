//! Self-update from GitHub Releases. Only does anything when running from a macOS .app bundle.
// ponytail: curl + ditto + codesign instead of an updater framework; move to Sparkle if delta updates ever matter.

use std::path::{Path, PathBuf};
use std::process::Command;

const API: &str = "https://api.github.com/repos/justin-r-wright/skillconstruct/releases/latest";
const DOWNLOADS: &str = "https://github.com/justin-r-wright/skillconstruct/releases/download/";

fn parse(v: &str) -> Vec<u32> {
    v.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

pub fn newer(latest: &str, current: &str) -> bool {
    parse(latest) > parse(current)
}

/// The .app this binary runs from, if any.
fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let app = exe.ancestors().nth(3)?;
    (app.extension()? == "app").then(|| app.to_path_buf())
}

/// (version, download url) of a newer release, if there is one.
pub fn check() -> Option<(String, String)> {
    bundle()?;
    let out = Command::new("curl").args(["-fsSL", "-m", "15", API]).output().ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let tag = v["tag_name"].as_str()?;
    let url = v["assets"]
        .as_array()?
        .iter()
        .filter_map(|a| a["browser_download_url"].as_str())
        .find(|u| u.starts_with(DOWNLOADS) && u.ends_with("-macos.zip"))?;
    newer(tag, env!("CARGO_PKG_VERSION")).then(|| (tag.trim_start_matches('v').to_string(), url.to_string()))
}

fn sh(cmd: &mut Command) -> Result<String, String> {
    let o = cmd.output().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    if o.status.success() {
        Ok(text)
    } else {
        Err(format!("{:?} failed: {}", cmd.get_program(), text.trim()))
    }
}

/// The Apple developer team that signed `app`, after checking the signature is intact.
fn team(app: &Path) -> Result<String, String> {
    sh(Command::new("codesign").args(["--verify", "--deep", "--strict"]).arg(app))?;
    let info = sh(Command::new("codesign").arg("-dv").arg(app))?;
    info.lines()
        .find_map(|l| l.strip_prefix("TeamIdentifier="))
        .filter(|t| *t != "not set")
        .map(str::to_owned)
        .ok_or_else(|| format!("{} is not signed by a developer team", app.display()))
}

/// Downloads the release, checks it is notarized and signed by the same team as the running app,
/// then swaps it in place. Returns the bundle to relaunch.
pub fn install(url: &str) -> Result<PathBuf, String> {
    let app = bundle().ok_or("not running from an app bundle")?;
    if !url.starts_with(DOWNLOADS) {
        return Err("unexpected download address".into());
    }
    let io = |e: std::io::Error| e.to_string();
    let tmp = std::env::temp_dir().join("skillconstruct-update");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(io)?;
    let zip = tmp.join("update.zip");
    sh(Command::new("curl").args(["-fsSL", "-m", "300", "-o"]).arg(&zip).arg(url))?;
    sh(Command::new("ditto").args(["-x", "-k"]).arg(&zip).arg(&tmp))?;
    let fresh = std::fs::read_dir(&tmp)
        .map_err(io)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "app"))
        .ok_or("the download has no app in it")?;

    if team(&fresh)? != team(&app)? {
        return Err("the update is signed by a different developer, not installing it".into());
    }
    sh(Command::new("spctl").args(["--assess", "--type", "execute"]).arg(&fresh))?;

    // Stage next to the running app so both renames stay on one volume.
    let (staged, old) = (app.with_extension("app.new"), app.with_extension("app.old"));
    let _ = std::fs::remove_dir_all(&staged);
    let _ = std::fs::remove_dir_all(&old);
    sh(Command::new("ditto").arg(&fresh).arg(&staged))?;
    std::fs::rename(&app, &old).map_err(|e| format!("cannot replace {} ({e}). Move the app to Applications first.", app.display()))?;
    if let Err(e) = std::fs::rename(&staged, &app) {
        let _ = std::fs::rename(&old, &app);
        return Err(e.to_string());
    }
    let _ = std::fs::remove_dir_all(&old);
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(app)
}

pub fn relaunch(app: &Path) -> ! {
    let _ = Command::new("open").arg("-n").arg(app).spawn();
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn newer() {
        assert!(super::newer("v0.2.0", "0.1.0"));
        assert!(super::newer("0.10.0", "0.9.9"));
        assert!(!super::newer("v0.1.0", "0.1.0"));
        assert!(!super::newer("v0.1.0", "0.2.0"));
    }
}
