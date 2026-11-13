//! Scanning and file operations. No UI in here so it can be tested against a fake home dir.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Root {
    pub label: &'static str,
    pub dir: PathBuf,
}

pub struct Env {
    pub home: PathBuf,
    /// Index 0 is where new skills are copied; the rest get symlinks.
    pub roots: Vec<Root>,
    pub trash: PathBuf,
}

impl Env {
    pub fn real() -> Env {
        Env::at(PathBuf::from(std::env::var_os("HOME").expect("HOME not set")))
    }

    pub fn at(home: PathBuf) -> Env {
        let roots = [
            ("shared", ".agents/skills"),
            ("claude", ".claude/skills"),
            ("codex", ".codex/skills"),
            ("cursor", ".cursor/skills"),
            ("opencode", ".config/opencode/skills"),
        ]
        .into_iter()
        .map(|(label, p)| Root { label, dir: home.join(p) })
        .collect();
        // ponytail: on Linux this drops folders into the XDG trash without a .trashinfo file, so
        // they are recoverable by hand but not listed by the file manager. Write .trashinfo if that matters.
        let trash = if cfg!(target_os = "macos") { ".Trash" } else { ".local/share/Trash/files" };
        Env { trash: home.join(trash), roots, home }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Kind {
    Dir,
    Link,
    Broken,
}

#[derive(Clone)]
pub struct Entry {
    pub kind: Kind,
    /// Fully resolved directory, None when the link is broken.
    pub real: Option<PathBuf>,
}

#[derive(Clone, Default)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// One slot per Env root.
    pub entries: Vec<Option<Entry>>,
    pub source: String,
    /// In ~/.agents/.skill-lock.json, so `npx skills update` can refresh it.
    pub tracked: bool,
    /// Owned by something else (a Claude Code plugin, or built into a harness): shown read-only.
    pub plugin: bool,
    pub issues: Vec<String>,
    pub dir: Option<PathBuf>,
    /// The skill really lives here, outside every skill folder: something else put it there and owns it.
    pub outside: Option<PathBuf>,
    /// Other installed skills this one's SKILL.md mentions, so removing them may break it.
    pub needs: Vec<String>,
    /// The reverse: skills whose SKILL.md mentions this one.
    pub needed_by: Vec<String>,
}

impl Skill {
    /// The copy other harnesses should link to: a real dir first, else whatever a link resolves to.
    pub fn canonical(&self) -> Option<&PathBuf> {
        let live = || self.entries.iter().flatten();
        live()
            .find(|e| e.kind == Kind::Dir)
            .or_else(|| live().find(|e| e.real.is_some()))
            .and_then(|e| e.real.as_ref())
    }
}

/// ponytail: not a YAML parser. Reads top-level `key: value` pairs plus indented
/// continuation lines, which is all SKILL.md frontmatter uses for name/description.
pub fn parse_frontmatter(text: &str) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    let Some(rest) = text.trim_start().strip_prefix("---") else { return map };
    let Some(end) = rest.find("\n---") else { return map };
    let mut cur: Option<String> = None;
    for line in rest[..end].lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(v) = cur.as_ref().and_then(|k| map.get_mut(k)) {
                if !v.is_empty() {
                    v.push(' ');
                }
                v.push_str(line.trim());
            }
        } else if let Some((k, v)) = line.split_once(':') {
            let v = v.trim();
            let v = if matches!(v, ">" | "|" | ">-" | "|-" | ">+" | "|+") { "" } else { v };
            map.insert(k.trim().to_string(), v.to_string());
            cur = Some(k.trim().to_string());
        }
    }
    for v in map.values_mut() {
        let t = v.trim();
        let quoted = t.len() >= 2
            && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')));
        // Double-quoted YAML strings escape their inner quotes.
        *v = if quoted { t[1..t.len() - 1].replace("\\\"", "\"") } else { t.to_string() };
    }
    map
}

fn lint(name: &str, md: Option<&str>, issues: &mut Vec<String>) -> String {
    let Some(md) = md else {
        issues.push("no SKILL.md".into());
        return String::new();
    };
    let fm = parse_frontmatter(md);
    let desc = fm.get("description").cloned().unwrap_or_default();
    match fm.get("name") {
        None => issues.push("frontmatter has no name".into()),
        Some(n) if n != name => issues.push(format!("frontmatter name '{n}' differs from folder name")),
        _ => {}
    }
    if desc.is_empty() {
        issues.push("frontmatter has no description (harnesses can't auto-trigger it)".into());
    } else if desc.chars().count() > 1024 {
        issues.push("description is over 1024 characters (some harnesses reject or truncate it)".into());
    }
    desc
}

pub fn scan(env: &Env) -> Vec<Skill> {
    let n = env.roots.len();
    let mut map: BTreeMap<String, Skill> = BTreeMap::new();
    for (i, root) in env.roots.iter().enumerate() {
        let Ok(rd) = fs::read_dir(&root.dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // ponytail: `synced` is Claude's own account-sync bucket, not ours to manage.
            if name.starts_with('.') || name == "synced" {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(e.path()) else { continue };
            let is_link = meta.file_type().is_symlink();
            if !is_link && !meta.is_dir() {
                continue;
            }
            let real = fs::canonicalize(e.path()).ok().filter(|p| p.is_dir());
            let kind = match (is_link, &real) {
                (false, _) => Kind::Dir,
                (true, Some(_)) => Kind::Link,
                (true, None) => Kind::Broken,
            };
            let s = map.entry(name.clone()).or_insert_with(|| Skill {
                name,
                entries: vec![None; n],
                ..Default::default()
            });
            s.entries[i] = Some(Entry { kind, real });
        }
    }

    let lock: serde_json::Value = fs::read_to_string(env.home.join(".agents/.skill-lock.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();

    let inside: Vec<PathBuf> = env.roots.iter().filter_map(|r| fs::canonicalize(&r.dir).ok()).collect();
    for s in map.values_mut() {
        let mut reals: Vec<PathBuf> = Vec::new();
        for (i, e) in s.entries.iter().enumerate() {
            let Some(e) = e else { continue };
            match &e.real {
                Some(r) if !reals.contains(r) => reals.push(r.clone()),
                None => s.issues.push(format!("broken symlink in {}", env.roots[i].label)),
                _ => {}
            }
        }
        s.dir = s.canonical().cloned();
        if let Some(dir) = &s.dir {
            let md = fs::read_to_string(dir.join("SKILL.md")).ok();
            s.description = lint(&s.name, md.as_deref(), &mut s.issues);
            let differs = reals.iter().any(|r| fs::read_to_string(r.join("SKILL.md")).ok() != md);
            if differs {
                s.issues.push(format!(
                    "{} separate copies whose SKILL.md differs: harnesses are running different versions",
                    reals.len()
                ));
            }
        }
        let src = lock["skills"][&s.name]["source"].as_str();
        s.tracked = src.is_some();
        s.outside = reals.into_iter().find(|r| !inside.iter().any(|root| r.starts_with(root)));
        s.source = match (src, &s.outside) {
            (Some(src), _) => src.to_string(),
            // Group by the folder the owner keeps its skills in, e.g. "linked from ~/.local/share/ego".
            (None, Some(o)) => format!("linked from {}", tilde(env, o.parent().unwrap_or(o))),
            (None, None) => "local".to_string(),
        };
    }
    map.into_values().collect()
}

/// Whether `text` refers to the skill `name`, not just a word that happens to match.
/// ponytail: a text heuristic, not a declared dependency. Hyphenated names count wherever they stand
/// alone; plain words like "research" only when written as `name`, /name, $name or "name skill".
/// Read a `requires:` frontmatter key instead if the skill format ever gets one.
pub fn mentions(text: &str, name: &str) -> bool {
    let part = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    text.match_indices(name).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = &text[i + name.len()..];
        let alone = !before.is_some_and(part) && !after.chars().next().is_some_and(part);
        let marked = before.is_some_and(|c| "`/$".contains(c)) || after.get(..6).is_some_and(|a| a.eq_ignore_ascii_case(" skill"));
        alone && (name.contains('-') || marked)
    })
}

/// Fills in `needs` for every skill in `all` from the removable skills named in `targets`.
pub fn link_needs(all: &mut [Skill], targets: &[String]) {
    for s in all.iter_mut() {
        let Some(md) = s.dir.as_ref().and_then(|d| fs::read_to_string(d.join("SKILL.md")).ok()) else { continue };
        s.needs = targets.iter().filter(|t| **t != s.name && mentions(&md, t)).cloned().collect();
    }
}

/// `p` with the home folder shortened to `~`.
pub fn tilde(env: &Env, p: &Path) -> String {
    let home = fs::canonicalize(&env.home).unwrap_or_else(|_| env.home.clone());
    p.strip_prefix(&home).map_or_else(|_| p.display().to_string(), |rest| format!("~/{}", rest.display()))
}

/// Percent of distinct non-empty lines two texts share, 100 meaning identical.
pub fn similarity(a: &str, b: &str) -> u8 {
    let lines = |t: &str| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect::<HashSet<_>>();
    let (a, b) = (lines(a), lines(b));
    match a.union(&b).count() {
        0 => 100,
        all => (a.intersection(&b).count() * 100 / all) as u8,
    }
}

/// Skills this app must not touch: shipped inside Claude Code plugins, or built into a harness
/// that maintains (and restores) them itself. Shown read-only.
pub fn plugin_skills(env: &Env) -> Vec<Skill> {
    let v: serde_json::Value = fs::read_to_string(env.home.join(".claude/plugins/installed_plugins.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut out = Vec::new();
    for (key, installs) in v["plugins"].as_object().into_iter().flatten() {
        let Some(path) = installs[0]["installPath"].as_str() else { continue };
        let plugin = key.split('@').next().unwrap_or(key);
        let Ok(rd) = fs::read_dir(Path::new(path).join("skills")) else { continue };
        for e in rd.flatten() {
            let Ok(md) = fs::read_to_string(e.path().join("SKILL.md")) else { continue };
            out.push(Skill {
                name: format!("{plugin}:{}", e.file_name().to_string_lossy()),
                description: parse_frontmatter(&md).remove("description").unwrap_or_default(),
                entries: vec![None; env.roots.len()],
                source: format!("plugin {key}"),
                plugin: true,
                dir: Some(e.path()),
                ..Default::default()
            });
        }
    }
    // Where Codex and Cursor keep the skills they ship and update themselves.
    for (label, sub) in [("codex", ".codex/skills/.system"), ("cursor", ".cursor/skills-cursor")] {
        let Ok(rd) = fs::read_dir(env.home.join(sub)) else { continue };
        for e in rd.flatten() {
            let Ok(md) = fs::read_to_string(e.path().join("SKILL.md")) else { continue };
            let mut entries = vec![None; env.roots.len()];
            if let Some(i) = env.roots.iter().position(|r| r.label == label) {
                entries[i] = Some(Entry { kind: Kind::Dir, real: Some(e.path()) });
            }
            out.push(Skill {
                name: format!("{label}:{}", e.file_name().to_string_lossy()),
                description: parse_frontmatter(&md).remove("description").unwrap_or_default(),
                entries,
                source: format!("built into {label}"),
                plugin: true,
                dir: Some(e.path()),
                ..Default::default()
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.starts_with('.') || name.contains('/') {
        return Err(format!("refusing odd skill name '{name}'"));
    }
    Ok(())
}

fn err(e: std::io::Error) -> String {
    e.to_string()
}

pub fn link(env: &Env, skill: &Skill, root: usize) -> Result<(), String> {
    check_name(&skill.name)?;
    let target = skill.canonical().ok_or("no working copy to link to")?;
    fs::create_dir_all(&env.roots[root].dir).map_err(err)?;
    symlink(target, env.roots[root].dir.join(&skill.name)).map_err(err)
}

/// Remove a skill from one root (`only`) or from all of them. Symlinks are deleted,
/// real folders are moved to the Trash, never deleted outright.
pub fn remove(env: &Env, name: &str, only: Option<usize>) -> Result<(), String> {
    check_name(name)?;
    let path = |i: usize| env.roots[i].dir.join(name);
    let is_link = |p: &Path| fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false);

    // Links can chain through each other (codex -> claude -> agents). Remember where each
    // one really lands so a survivor can be re-pointed instead of left dangling.
    let survivors: Vec<(PathBuf, PathBuf)> = (0..env.roots.len())
        .filter(|i| only.is_some_and(|o| o != *i) && is_link(&path(*i)))
        .filter_map(|i| Some((path(i), fs::canonicalize(path(i)).ok()?)))
        .collect();

    for i in (0..env.roots.len()).filter(|i| only.is_none_or(|o| o == *i)) {
        let p = path(i);
        if is_link(&p) {
            fs::remove_file(&p).map_err(err)?;
        } else if p.is_dir() {
            let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            fs::create_dir_all(&env.trash).map_err(err)?;
            // The harness label keeps two real copies of one skill from colliding in the Trash.
            fs::rename(&p, env.trash.join(format!("{name}-{}-{ts}", env.roots[i].label)))
                .map_err(|e| format!("could not move {} to Trash: {e}", p.display()))?;
        }
    }

    for (p, real) in survivors {
        if fs::metadata(&p).is_ok() {
            continue;
        }
        fs::remove_file(&p).map_err(err)?;
        if real.is_dir() {
            symlink(&real, &p).map_err(err)?;
        }
    }
    if !(0..env.roots.len()).any(|i| fs::symlink_metadata(path(i)).is_ok()) {
        forget(env, name)?;
    }
    Ok(())
}

/// Drop a skill from the `npx skills` lock file, otherwise `npx skills update` reinstalls it.
fn forget(env: &Env, name: &str) -> Result<(), String> {
    let lock_path = env.home.join(".agents/.skill-lock.json");
    let Ok(text) = fs::read_to_string(&lock_path) else { return Ok(()) };
    let mut lock: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("removed, but could not read the lock file: {e}"))?;
    let gone = lock["skills"].as_object_mut().and_then(|m| m.remove(name)).is_some();
    if gone {
        let out = serde_json::to_string_pretty(&lock).map_err(|e| e.to_string())?;
        fs::write(&lock_path, out + "\n").map_err(err)?;
    }
    Ok(())
}

/// Copy a local skill folder into the first root and link it into every other harness dir that exists.
pub fn add_local(env: &Env, src: &Path) -> Result<String, String> {
    let src = fs::canonicalize(src).map_err(err)?;
    if !src.join("SKILL.md").is_file() {
        return Err(format!("{} has no SKILL.md", src.display()));
    }
    let name = src.file_name().ok_or("bad path")?.to_string_lossy().into_owned();
    check_name(&name)?;
    if env.roots.iter().any(|r| fs::symlink_metadata(r.dir.join(&name)).is_ok()) {
        return Err(format!("a skill named '{name}' is already installed"));
    }
    let dest = env.roots[0].dir.join(&name);
    fs::create_dir_all(&env.roots[0].dir).map_err(err)?;
    let ok = Command::new("cp").arg("-R").arg(&src).arg(&dest).status().map_err(err)?.success();
    if !ok {
        return Err("cp failed".into());
    }
    let mut linked = vec![env.roots[0].label];
    for r in env.roots[1..].iter().filter(|r| r.dir.is_dir()) {
        symlink(&dest, r.dir.join(&name)).map_err(err)?;
        linked.push(r.label);
    }
    Ok(format!("installed '{name}' into: {}", linked.join(", ")))
}

pub type Usage = HashMap<String, (u32, SystemTime)>;

fn bump(out: &mut Usage, name: &str, when: SystemTime) {
    let slot = out.entry(name.to_string()).or_insert((0, UNIX_EPOCH));
    slot.0 += 1;
    slot.1 = slot.1.max(when);
}

/// Runs `f` on the text and modified time of every .jsonl transcript under `dir`.
/// ponytail: reads every transcript on each scan (seconds per GB). Cache on file mtime if this gets used daily.
fn each_transcript(dir: &Path, f: &mut dyn FnMut(&str, SystemTime)) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            each_transcript(&p, f);
        } else if ft.is_file() && p.extension().is_some_and(|x| x == "jsonl") {
            let Ok(bytes) = fs::read(&p) else { continue };
            let when = e.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
            f(&String::from_utf8_lossy(&bytes), when);
        }
    }
}

/// Count Skill tool calls and typed /slash commands in Claude Code transcripts under `dir`.
pub fn scan_usage(dir: &Path, out: &mut Usage) {
    // A Skill tool call the model made, and a /slash command the user typed.
    const NEEDLES: [(&str, char); 2] =
        [(r#""name":"Skill","input":{"skill":""#, '"'), ("<command-name>/", '<')];
    each_transcript(dir, &mut |text, when| {
        for (needle, end) in NEEDLES {
            for (idx, _) in text.match_indices(needle) {
                let rest = &text[idx + needle.len()..];
                if let Some(name) = rest.split(end).next().filter(|n| !n.is_empty() && n.len() < 100) {
                    bump(out, name, when);
                }
            }
        }
    });
}

/// Count skills Codex opened in the session transcripts under `dir`.
/// ponytail: Codex has no skill tool, so "used" means a command it ran named `skills/<name>/SKILL.md`.
/// A search that merely prints that path counts too; parse the command if that inflates numbers.
pub fn scan_codex_usage(dir: &Path, out: &mut Usage) {
    const CALLS: [&str; 3] =
        [r#""type":"custom_tool_call","#, r#""type":"function_call","#, r#""type":"local_shell_call","#];
    each_transcript(dir, &mut |text, when| {
        for line in text.lines().filter(|l| CALLS.iter().any(|c| l.contains(c))) {
            let mut seen: Vec<&str> = Vec::new();
            for (idx, _) in line.match_indices("/SKILL.md") {
                let Some((before, name)) = line[..idx].rsplit_once('/') else { continue };
                let plain = !name.is_empty() && name.len() < 100 && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
                if plain && before.ends_with("skills") && !seen.contains(&name) {
                    seen.push(name);
                    bump(out, name, when);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_home(tag: &str) -> Env {
        let home = std::env::temp_dir().join(format!("skillconstruct-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let env = Env::at(home);
        for r in &env.roots[..3] {
            fs::create_dir_all(&r.dir).unwrap();
        }
        env
    }

    fn write_skill(dir: &Path, body: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("SKILL.md"), body).unwrap();
    }

    #[test]
    fn frontmatter() {
        let fm = parse_frontmatter("---\nname: tdd\ndescription: >\n  Test first.\n  Always.\nmetadata:\n  type: x\n---\n# Body\nname: nope\n");
        assert_eq!(fm["name"], "tdd");
        assert_eq!(fm["description"], "Test first. Always.");
        assert_eq!(parse_frontmatter("---\ndescription: \"Use: when asked\"\n---\n")["description"], "Use: when asked");
        assert!(parse_frontmatter("# no frontmatter").is_empty());
    }

    #[test]
    fn scan_lint_and_remove() {
        let env = fake_home("ops");
        let (agents, claude, codex) = (&env.roots[0].dir, &env.roots[1].dir, &env.roots[2].dir);
        // tdd: real in agents, claude links to it, codex links through claude (a chain).
        write_skill(&agents.join("tdd"), "---\nname: tdd\ndescription: Test first.\n---\n");
        symlink(agents.join("tdd"), claude.join("tdd")).unwrap();
        symlink(claude.join("tdd"), codex.join("tdd")).unwrap();
        // plan: two real copies that drifted apart, and the claude one has a bad name.
        write_skill(&claude.join("plan"), "---\nname: planner\ndescription: a\n---\n");
        write_skill(&codex.join("plan"), "---\nname: plan\ndescription: b\n---\n");
        symlink(agents.join("gone"), claude.join("ghost")).unwrap();

        let skills = scan(&env);
        let get = |n: &str| skills.iter().find(|s| s.name == n).unwrap();
        let tdd = get("tdd");
        let kinds: Vec<_> = tdd.entries.iter().map(|e| e.as_ref().map(|e| e.kind.clone())).collect();
        assert_eq!(kinds, [Some(Kind::Dir), Some(Kind::Link), Some(Kind::Link), None, None]);
        assert!(tdd.issues.is_empty(), "{:?}", tdd.issues);
        assert_eq!(tdd.description, "Test first.");
        assert_eq!(get("plan").issues.len(), 2, "{:?}", get("plan").issues);
        assert_eq!(get("ghost").issues, ["broken symlink in claude"]);

        // Unlinking the middle of the chain must not take codex down with it.
        remove(&env, "tdd", Some(1)).unwrap();
        assert!(fs::symlink_metadata(claude.join("tdd")).is_err());
        assert!(codex.join("tdd/SKILL.md").is_file());

        // Linking it back, then removing everywhere, trashes the one real copy.
        link(&env, get("tdd"), 1).unwrap();
        assert!(claude.join("tdd/SKILL.md").is_file());
        let lock = env.home.join(".agents/.skill-lock.json");
        fs::write(&lock, r#"{"version":3,"skills":{"tdd":{"source":"a/b"},"keep":{"source":"c/d"}},"dismissed":{}}"#).unwrap();
        remove(&env, "tdd", None).unwrap();
        // The lock entry goes too, and nothing else in the file is touched or reordered.
        let after = fs::read_to_string(&lock).unwrap();
        assert!(!after.contains("tdd") && after.contains("keep"), "{after}");
        assert!(after.find("version").unwrap() < after.find("skills").unwrap());
        for r in &env.roots {
            assert!(fs::symlink_metadata(r.dir.join("tdd")).is_err());
        }
        let trashed: Vec<_> = fs::read_dir(&env.trash).unwrap().flatten().collect();
        assert_eq!(trashed.len(), 1);
        assert!(trashed[0].path().join("SKILL.md").is_file());

        // Trashing the only real copy from one root also clears links that pointed at it.
        write_skill(&agents.join("solo"), "---\nname: solo\ndescription: x\n---\n");
        symlink(agents.join("solo"), codex.join("solo")).unwrap();
        remove(&env, "solo", Some(0)).unwrap();
        assert!(fs::symlink_metadata(codex.join("solo")).is_err());

        assert!(remove(&env, "../x", None).is_err());

        // Onboarding a local folder copies it once and links it into existing harness dirs.
        let src = env.home.join("incoming/fresh");
        write_skill(&src, "---\nname: fresh\ndescription: new\n---\n");
        add_local(&env, &src).unwrap();
        assert!(!fs::symlink_metadata(agents.join("fresh")).unwrap().file_type().is_symlink());
        assert!(claude.join("fresh/SKILL.md").is_file() && codex.join("fresh/SKILL.md").is_file());
        assert!(add_local(&env, &src).is_err());

        let _ = fs::remove_dir_all(&env.home);
    }

    #[test]
    fn remove_two_real_copies() {
        let env = fake_home("twocopies");
        write_skill(&env.roots[1].dir.join("dup"), "---\nname: dup\ndescription: a\n---\n");
        write_skill(&env.roots[2].dir.join("dup"), "---\nname: dup\ndescription: b\n---\n");
        remove(&env, "dup", None).unwrap();
        assert!(scan(&env).is_empty());
        assert_eq!(fs::read_dir(&env.trash).unwrap().count(), 2);
    }

    #[test]
    fn usage() {
        let env = fake_home("usage");
        let dir = env.home.join("projects/a/sub");
        fs::create_dir_all(&dir).unwrap();
        let call = r#"{"type":"tool_use","name":"Skill","input":{"skill":"tdd"}}"#;
        fs::write(dir.join("t.jsonl"), format!("{call}\n{call}\n{{\"name\":\"Skill\",\"description\":\"x\"}}\n<command-name>/tdd</command-name>\n")).unwrap();
        let mut u = Usage::new();
        scan_usage(&env.home.join("projects"), &mut u);
        assert_eq!(u.len(), 1);
        assert_eq!(u["tdd"].0, 3);
        let _ = fs::remove_dir_all(&env.home);
    }

    #[test]
    fn codex_usage() {
        let env = fake_home("codex-usage");
        let dir = env.home.join("sessions");
        fs::create_dir_all(&dir).unwrap();
        let call = r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","input":"cat ~/.agents/skills/tdd/SKILL.md ~/.agents/skills/tdd/SKILL.md ~/.codex/skills/.system/imagegen/SKILL.md"}}"#;
        let echo = r#"{"type":"response_item","payload":{"type":"custom_tool_call_output","output":"skills/tdd/SKILL.md"}}"#;
        fs::write(dir.join("a.jsonl"), format!("{call}\n{echo}\n")).unwrap();
        let mut u = Usage::new();
        scan_codex_usage(&dir, &mut u);
        assert_eq!(u.len(), 1);
        assert_eq!(u["tdd"].0, 1);
    }

    #[test]
    fn outside_and_similarity() {
        let env = fake_home("outside");
        let theirs = env.home.join("apps/ego/skills-src");
        write_skill(&theirs, "---\nname: ego\ndescription: d\n---\n");
        write_skill(&env.roots[0].dir.join("mine"), "---\nname: mine\ndescription: d\n---\n");
        symlink(&theirs, env.roots[1].dir.join("ego")).unwrap();
        let skills = scan(&env);
        let ego = skills.iter().find(|s| s.name == "ego").unwrap();
        assert!(ego.outside.is_some());
        assert_eq!(ego.source, "linked from ~/apps/ego");
        assert!(skills.iter().find(|s| s.name == "mine").unwrap().outside.is_none());

        assert_eq!(similarity("a\nb\n", " a\n\nb"), 100);
        assert_eq!(similarity("a\nb", "a\nc"), 33);
        assert_eq!(similarity("a", "b"), 0);
    }

    #[test]
    fn needs() {
        assert!(mentions("then run the watch-pr skill", "watch-pr"));
        assert!(!mentions("use ship-and-watch-pr here", "watch-pr"));
        assert!(!mentions("do some research first", "research"));
        assert!(mentions("load `research` first", "research"));
        assert!(mentions("use the research skill", "research"));
        assert!(mentions("type /tdd to start", "tdd"));

        let env = fake_home("needs");
        write_skill(&env.roots[0].dir.join("ship"), "---\nname: ship\ndescription: d\n---\nUses the watch-pr skill.\n");
        write_skill(&env.roots[0].dir.join("watch-pr"), "---\nname: watch-pr\ndescription: d\n---\n");
        let mut skills = scan(&env);
        let names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
        link_needs(&mut skills, &names);
        assert_eq!(skills.iter().find(|s| s.name == "ship").unwrap().needs, ["watch-pr"]);
        assert!(skills.iter().find(|s| s.name == "watch-pr").unwrap().needs.is_empty());
    }
}
