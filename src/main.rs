mod core;
mod update;

use core::{Env, Kind, Skill, Usage};
use eframe::egui::{
    self, vec2, Align, Align2, Color32, CornerRadius, FontId, Label, Layout, Margin, Rect, RichText, Sense, Stroke,
    UiBuilder,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::SystemTime;

const AGENTS: [&str; 3] = ["claude", "codex", "opencode"];
/// Remembered model and thinking level per agent, under the home folder: one `agent=model level` line each.
const MODELS_FILE: &str = ".config/skillconstruct/models";

const BG: Color32 = Color32::from_rgb(15, 17, 20);
const PANEL: Color32 = Color32::from_rgb(20, 23, 27);
const CARD: Color32 = Color32::from_rgb(29, 33, 38);
const BORDER: Color32 = Color32::from_rgb(42, 47, 54);
const TEXT: Color32 = Color32::from_rgb(228, 231, 235);
const MUTED: Color32 = Color32::from_rgb(134, 143, 153);
const ACCENT: Color32 = Color32::from_rgb(96, 156, 245);
const SELECTED: Color32 = Color32::from_rgb(33, 48, 72);
const RED: Color32 = Color32::from_rgb(235, 100, 100);
const AMBER: Color32 = Color32::from_rgb(232, 172, 76);
const GREEN: Color32 = Color32::from_rgb(104, 200, 132);

const ROW_H: f32 = 48.0;
const HARNESS_W: f32 = 58.0;
const USES_W: f32 = 104.0;
const SOURCE_W: f32 = 160.0;

/// A double-clicked app gets a bare PATH without npx, claude or codex on it, so borrow the login shell's.
fn adopt_shell_path() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let out = Command::new(shell).args(["-lic", "printf '\\n%s' \"$PATH\""]).stdin(Stdio::null()).output();
    // Last line only: interactive shells may print a banner first.
    if let Some(path) = out.ok().and_then(|o| String::from_utf8(o.stdout).ok()?.lines().last().map(str::to_owned)) {
        if path.contains('/') {
            let current = std::env::var("PATH").unwrap_or_default();
            std::env::set_var("PATH", format!("{path}:{current}"));
        }
    }
}

fn main() -> eframe::Result {
    adopt_shell_path();
    let options = eframe::NativeOptions {
        // An empty icon stops eframe swapping its own default into the Dock, so the bundle's icon stays.
        viewport: egui::ViewportBuilder::default().with_icon(egui::IconData::default()).with_inner_size([1760.0, 980.0]).with_min_inner_size([1100.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Skill Construct",
        options,
        Box::new(|cc| {
            setup_look(&cc.egui_ctx);
            Ok(Box::new(App::new(&cc.egui_ctx)))
        }),
    )
}

fn setup_look(ctx: &egui::Context) {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    // The macOS system font; silently keep egui's built-in font anywhere it is missing.
    if let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNS.ttf") {
        ctx.add_font(FontInsert {
            name: "system".into(),
            data: egui::FontData::from_owned(bytes),
            families: vec![InsertFontFamily { family: egui::FontFamily::Proportional, priority: FontPriority::Highest }],
        });
    }
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|s| {
        use egui::TextStyle::*;
        s.text_styles.insert(Body, FontId::proportional(14.5));
        s.text_styles.insert(Button, FontId::proportional(13.5));
        s.text_styles.insert(Heading, FontId::proportional(22.0));
        s.text_styles.insert(Small, FontId::proportional(12.0));
        s.text_styles.insert(Monospace, FontId::monospace(12.5));
        s.spacing.item_spacing = vec2(8.0, 7.0);
        s.spacing.button_padding = vec2(11.0, 5.0);
        s.spacing.interact_size.y = 28.0;
        let v = &mut s.visuals;
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(12);
        v.extreme_bg_color = Color32::from_rgb(10, 11, 13);
        v.selection.bg_fill = SELECTED;
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.widgets.noninteractive.fg_stroke.color = TEXT;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = CornerRadius::same(7);
            w.fg_stroke.color = TEXT;
        }
        v.widgets.noninteractive.corner_radius = CornerRadius::same(7);
        v.widgets.inactive.weak_bg_fill = CARD;
        v.widgets.inactive.bg_fill = CARD;
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(40, 46, 53);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER);
    });
}

enum Msg {
    /// `relink`: a skill to link back into these harnesses once the job is done.
    Job { title: String, out: String, relink: Option<(String, Vec<usize>)> },
    Matches(Vec<(String, Vec<Hit>)>),
    Text(Hit, Option<String>),
    Usage(Usage),
    Answer { skill: String, who: &'static str, text: String },
    Models(usize, Vec<(String, Vec<String>)>),
    Hits(Result<Vec<Hit>, String>),
    UpdateAvailable(Option<(String, String)>),
    Updated(Result<PathBuf, String>),
}

/// One search result from skills.sh.
#[derive(Clone)]
struct Hit {
    source: String,
    skill: String,
    name: String,
    installs: u64,
    /// How much of its SKILL.md matches the local copy, 0-100. None until compared.
    same: Option<u8>,
}

/// The SKILL.md a skills.sh result points at, read from its public GitHub repo.
// ponytail: unauthenticated GitHub API, 60 repo listings an hour. Cache per repo if lookups get rate-limited.
fn remote_md(source: &str, skill: &str) -> Option<String> {
    let ok = |part: &str| !part.is_empty() && !part.starts_with('.') && part.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    let (owner, repo) = source.split_once('/')?;
    if !ok(owner) || !ok(repo) {
        return None;
    }
    let get = |url: String| {
        let out = Command::new("curl").args(["-fsSL", "-m", "15"]).arg(url).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let tree: serde_json::Value =
        serde_json::from_str(&get(format!("https://api.github.com/repos/{owner}/{repo}/git/trees/HEAD?recursive=1"))?).ok()?;
    let wanted = format!("{skill}/SKILL.md");
    let path = tree["tree"]
        .as_array()?
        .iter()
        .filter_map(|e| e["path"].as_str())
        .filter(|p| *p == wanted || p.ends_with(&format!("/{wanted}")))
        .min_by_key(|p| p.len())?;
    get(format!("https://raw.githubusercontent.com/{owner}/{repo}/HEAD/{path}"))
}

/// Searches the same public endpoint `npx skills find` uses.
fn search(q: &str) -> Result<Vec<Hit>, String> {
    let out = Command::new("curl")
        .args(["-fsSG", "-m", "15", "https://skills.sh/api/search", "-d", "limit=50", "--data-urlencode"])
        .arg(format!("q={q}"))
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let hit = |s: &serde_json::Value| {
        Some(Hit {
            source: s["source"].as_str()?.into(),
            skill: s["skillId"].as_str()?.into(),
            name: s["name"].as_str()?.into(),
            installs: s["installs"].as_u64().unwrap_or(0),
            same: None,
        })
    };
    Ok(v["skills"].as_array().into_iter().flatten().filter_map(hit).collect())
}

fn compact(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

#[derive(Clone, PartialEq)]
enum View {
    All,
    Issues,
    Unused,
    Plugins,
    Discover,
    Source(String),
}

/// Things the UI asked for this frame; applied after drawing so the draw code only reads state.
enum Todo {
    Select(String),
    Link(String, usize),
    ConfirmRemove(Vec<String>, Option<usize>),
    Remove(String, Option<usize>),
    Update(Option<String>),
    Add,
    Ask(String),
    Open(PathBuf),
    Rescan,
    ScanUsage,
    Search,
    Install(String, String),
    InstallUpdate,
    /// Look these skill names up on skills.sh.
    Lookup(Vec<String>),
    /// Swap a local skill for the copy at this source so it can update.
    Track(String, String),
    /// Read a skills.sh result in full. The name is the local skill it would replace, or "" when browsing.
    Read(String, Hit),
}

struct App {
    env: Env,
    skills: Vec<Skill>,
    plugin_skills: Vec<Skill>,
    usage: Option<Usage>,
    selected: Option<String>,
    /// Ticked rows, for removing a hand-picked set.
    checked: HashSet<String>,
    view: View,
    filter: String,
    sort_by_uses: bool,
    add_src: String,
    confirm: Option<(Vec<String>, Option<usize>)>,
    /// The skills.sh result being read: local skill name (or ""), the result, its text once loaded.
    preview: Option<(String, Hit, Option<String>)>,
    agent: usize,
    /// Model per agent; empty means the agent's own default.
    models: [String; 3],
    /// Thinking level per agent; empty means the agent's own default.
    efforts: [String; 3],
    /// Per agent: (model, thinking levels it supports; empty = the agent's generic set).
    model_lists: [Vec<(String, Vec<String>)>; 3],
    model_filter: String,
    chat_open: bool,
    market_q: String,
    hits: Vec<Hit>,
    /// Untracked skills looked up on skills.sh: same-named skills there, most installed first.
    matches: HashMap<String, Vec<Hit>>,
    searching: bool,
    searched: bool,
    update: Option<(String, String)>,
    updating: bool,
    question: String,
    /// Per skill (or "" for the whole library): (who said it, text).
    chats: HashMap<String, Vec<(&'static str, String)>>,
    asking: Option<String>,
    log: String,
    log_open: bool,
    busy: usize,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

fn run(cmd: &mut Command) -> String {
    match cmd.stdin(Stdio::null()).env("NO_COLOR", "1").output() {
        Err(e) => format!("could not start {:?}: {e}", cmd.get_program()),
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !o.status.success() || s.is_empty() {
                s = format!("{s}\n{}", String::from_utf8_lossy(&o.stderr)).trim().to_string();
            }
            strip_ansi(&s)
        }
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip a CSI sequence: ESC [ ... final letter.
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn days_ago(t: SystemTime) -> String {
    match SystemTime::now().duration_since(t).map(|d| d.as_secs() / 86400) {
        Ok(0) | Err(_) => "today".into(),
        Ok(d) => format!("{d}d ago"),
    }
}

fn caption(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).size(11.0).color(MUTED).strong());
}

/// A left-aligned, never-wrapping label inside `rect` that does not eat clicks meant for the row under it.
fn text_in(ui: &mut egui::Ui, rect: Rect, text: RichText) {
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
    child.add(Label::new(text).truncate().selectable(false));
}

/// Filled = a real copy lives here, ring = a link to a copy elsewhere, red = broken link, speck = absent.
fn status_dot(ui: &egui::Ui, center: egui::Pos2, kind: Option<&Kind>) {
    let p = ui.painter();
    match kind {
        Some(Kind::Dir) => p.circle_filled(center, 5.0, GREEN),
        Some(Kind::Link) => p.circle_stroke(center, 4.5, Stroke::new(1.6, ACCENT)),
        Some(Kind::Broken) => p.circle_filled(center, 5.0, RED),
        None => p.circle_filled(center, 1.5, BORDER),
    };
}

fn kind_word(kind: Option<&Kind>) -> &'static str {
    match kind {
        Some(Kind::Dir) => "real copy",
        Some(Kind::Link) => "link",
        Some(Kind::Broken) => "broken link",
        None => "not installed",
    }
}

fn nav_item(ui: &mut egui::Ui, selected: bool, label: &str, count: usize) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 29.0), Sense::click());
    let fill = if selected {
        SELECTED
    } else if resp.contains_pointer() {
        CARD
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 7.0, fill);
    let mut text_rect = rect.shrink2(vec2(10.0, 0.0));
    text_rect.max.x -= 34.0;
    text_in(ui, text_rect, RichText::new(label).color(if selected { TEXT } else { MUTED }).size(13.5));
    ui.painter().text(
        rect.right_center() - vec2(10.0, 0.0),
        Align2::RIGHT_CENTER,
        count.to_string(),
        FontId::proportional(12.0),
        MUTED,
    );
    resp.clicked()
}

impl App {
    fn new(ctx: &egui::Context) -> App {
        let env = Env::real();
        let (tx, rx) = channel();
        let mut models: [String; 3] = Default::default();
        let mut efforts: [String; 3] = Default::default();
        for line in std::fs::read_to_string(env.home.join(MODELS_FILE)).unwrap_or_default().lines() {
            if let Some((agent, model)) = line.split_once('=') {
                if let Some(i) = AGENTS.iter().position(|a| *a == agent) {
                    let (model, effort) = model.split_once(' ').unwrap_or((model, ""));
                    (models[i], efforts[i]) = (model.to_string(), effort.to_string());
                }
            }
        }
        let mut app = App {
            skills: Vec::new(),
            plugin_skills: Vec::new(),
            usage: None,
            selected: None,
            checked: HashSet::new(),
            view: View::All,
            filter: String::new(),
            sort_by_uses: false,
            add_src: String::new(),
            confirm: None,
            preview: None,
            agent: 0,
            models,
            efforts,
            model_lists: [["fable", "opus", "sonnet", "haiku"].map(|m| (m.to_string(), Vec::new())).to_vec(), Vec::new(), Vec::new()],
            model_filter: String::new(),
            chat_open: true,
            market_q: String::new(),
            hits: Vec::new(),
            matches: HashMap::new(),
            searching: false,
            searched: false,
            update: None,
            updating: false,
            question: String::new(),
            chats: HashMap::new(),
            asking: None,
            log: "Ready.".into(),
            log_open: false,
            busy: 0,
            env,
            tx,
            rx,
        };
        app.rescan();
        app.spawn(ctx, || Msg::UpdateAvailable(update::check()));
        app.spawn(ctx, || {
            let out = run(Command::new("opencode").arg("models"));
            Msg::Models(2, out.lines().filter(|l| l.contains('/') && !l.contains(' ')).map(|l| (l.to_string(), Vec::new())).collect())
        });
        app.spawn(ctx, || {
            // Codex prints its own catalog, including which thinking levels each model takes.
            let out = Command::new("codex").args(["debug", "models"]).stdin(Stdio::null()).output().map(|o| o.stdout).unwrap_or_default();
            let v: serde_json::Value = serde_json::from_slice(&out).unwrap_or_default();
            let model = |m: &serde_json::Value| {
                let levels = m["supported_reasoning_levels"].as_array().into_iter().flatten();
                (m["visibility"] == "list").then_some(())?;
                Some((m["slug"].as_str()?.to_string(), levels.filter_map(|l| Some(l["effort"].as_str()?.to_string())).collect()))
            };
            Msg::Models(1, v["models"].as_array().into_iter().flatten().filter_map(model).collect())
        });
        app
    }

    fn rescan(&mut self) {
        self.skills = core::scan(&self.env);
        self.plugin_skills = core::plugin_skills(&self.env);
        self.checked.retain(|n| self.skills.iter().any(|s| &s.name == n));
    }

    fn find(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().chain(&self.plugin_skills).find(|s| s.name == name)
    }

    fn spawn(&mut self, ctx: &egui::Context, f: impl FnOnce() -> Msg + Send + 'static) {
        self.busy += 1;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
    }

    fn job(&mut self, ctx: &egui::Context, title: String, cmd: Command) {
        self.job_then(ctx, title, cmd, None);
    }

    fn job_then(&mut self, ctx: &egui::Context, title: String, mut cmd: Command, relink: Option<(String, Vec<usize>)>) {
        self.log = format!("{title}\nrunning...");
        self.spawn(ctx, move || Msg::Job { out: run(&mut cmd), title, relink });
    }

    fn report(&mut self, what: &str, r: Result<(), String>) {
        self.log = match r {
            Ok(()) => format!("{what}: done"),
            Err(e) => format!("{what}: FAILED: {e}"),
        };
        self.rescan();
    }

    fn uses(&self, name: &str) -> Option<(u32, SystemTime)> {
        self.usage.as_ref().map(|u| u.get(name).copied().unwrap_or((0, SystemTime::UNIX_EPOCH)))
    }

    fn in_view(&self, view: &View, s: &Skill) -> bool {
        match view {
            View::Discover => false,
            View::Plugins => s.plugin,
            _ if s.plugin => false,
            View::All => true,
            View::Issues => !s.issues.is_empty(),
            View::Unused => matches!(self.uses(&s.name), Some((0, _))),
            View::Source(src) => &s.source == src,
        }
    }

    /// Thinking levels the picked model of agent `a` accepts.
    fn levels(&self, a: usize) -> Vec<String> {
        let generic: &[&str] = match AGENTS[a] {
            "claude" => &["low", "medium", "high", "xhigh", "max"],
            "codex" => &["low", "medium", "high", "xhigh"],
            // ponytail: opencode calls these variants and they differ per provider; this is the common set.
            _ => &["minimal", "low", "medium", "high", "max"],
        };
        match self.model_lists[a].iter().find(|(m, l)| *m == self.models[a] && !l.is_empty()) {
            Some((_, l)) => l.clone(),
            None => generic.iter().map(|l| l.to_string()).collect(),
        }
    }

    /// `key` is a skill name, or "" to ask about the whole library.
    fn prompt(&self, key: &str, question: &str) -> String {
        let intro = "You are helping me audit the agent skills installed on my computer. Answer directly and \
                     briefly, with a clear keep / remove / merge recommendation when asked. Plain text only, no \
                     markdown. Do not use tools or change anything.";
        let others: String = self
            .skills
            .iter()
            .chain(&self.plugin_skills)
            .filter(|s| s.name != key)
            .map(|s| format!("- {} [{}]: {}\n", s.name, s.source, s.description.chars().take(200).collect::<String>()))
            .collect();
        let so_far: String = self
            .chats
            .get(key)
            .map(|c| c.iter().map(|(who, text)| format!("{who}: {text}\n")).collect())
            .unwrap_or_else(|| "(none)".into());
        let tail = format!("=== Conversation so far ===\n{so_far}\n\nMy question: {question}");
        let Some(skill) = self.find(key) else {
            return format!("{intro}\n\n=== My skills ===\n{others}\n{tail}");
        };
        let md = skill
            .dir
            .as_ref()
            .and_then(|d| std::fs::read_to_string(d.join("SKILL.md")).ok())
            .unwrap_or_else(|| "(SKILL.md could not be read)".into());
        let installed: Vec<&str> = skill
            .entries
            .iter()
            .zip(&self.env.roots)
            .filter(|(e, _)| e.is_some())
            .map(|(_, r)| r.label)
            .collect();
        let usage = match self.uses(&skill.name) {
            Some((0, _)) => "0 invocations found in Claude Code transcripts".into(),
            Some((n, t)) => format!("{n} invocations in Claude Code transcripts, last {}", days_ago(t)),
            None => "usage not scanned".into(),
        };
        format!(
            "{intro}\n\n\
             Skill under review: {}\nSource: {}\nInstalled for: {}\nUsage: {usage}\nKnown issues: {}\n\n\
             === SKILL.md ===\n{}\n\n=== My other skills ===\n{others}\n{tail}",
            skill.name,
            skill.source,
            if installed.is_empty() { "plugin only".to_string() } else { installed.join(", ") },
            if skill.issues.is_empty() { "none".to_string() } else { skill.issues.join("; ") },
            md.chars().take(8000).collect::<String>(),
        )
    }

    fn apply(&mut self, ctx: &egui::Context, todo: Todo) {
        match todo {
            Todo::Select(name) => self.selected = Some(name),
            Todo::Rescan => {
                self.rescan();
                self.log = "Rescanned.".into();
            }
            Todo::ConfirmRemove(names, only) => self.confirm = Some((names, only)),
            Todo::Open(dir) => {
                let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
                let _ = Command::new(opener).arg(dir).spawn();
            }
            Todo::Link(name, root) => {
                let r = self.find(&name).ok_or("gone".to_string()).and_then(|s| core::link(&self.env, s, root));
                self.report(&format!("link {name} into {}", self.env.roots[root].label), r);
            }
            Todo::Remove(name, only) => {
                let r = core::remove(&self.env, &name, only);
                let scope = only.map_or("everywhere", |i| self.env.roots[i].label);
                self.report(&format!("remove {name} ({scope})"), r);
            }
            Todo::Update(name) => {
                let mut cmd = Command::new("npx");
                cmd.args(["-y", "skills", "update"]).args(&name).args(["-g", "-y"]);
                self.job(ctx, format!("npx skills update {}", name.unwrap_or_default()), cmd);
            }
            Todo::Add => {
                let src = self.add_src.trim().to_string();
                if src.is_empty() || src.starts_with('-') {
                    self.log = "add: enter a folder path, owner/repo, or URL".into();
                    return;
                }
                let local = PathBuf::from(src.replacen('~', &self.env.home.to_string_lossy(), 1));
                if local.is_dir() {
                    let r = core::add_local(&self.env, &local);
                    self.log = r.unwrap_or_else(|e| format!("add FAILED: {e}"));
                    self.rescan();
                } else {
                    let mut cmd = Command::new("npx");
                    cmd.args(["-y", "skills", "add", &src, "-g", "-y"]);
                    self.job(ctx, format!("npx skills add {src}"), cmd);
                }
                self.add_src.clear();
            }
            Todo::ScanUsage => {
                let (claude, codex) = (self.env.home.join(".claude/projects"), self.env.home.join(".codex/sessions"));
                self.log = "Scanning Claude Code and Codex transcripts for skill usage...".into();
                self.spawn(ctx, move || {
                    let mut u = Usage::new();
                    core::scan_usage(&claude, &mut u);
                    core::scan_codex_usage(&codex, &mut u);
                    Msg::Usage(u)
                });
            }
            Todo::Ask(key) => {
                let q = self.question.trim().to_string();
                let prompt = self.prompt(&key, &q);
                let who = AGENTS[self.agent];
                let (model, effort) = (self.models[self.agent].clone(), self.efforts[self.agent].clone());
                let opt = |flag: &str, value: &str| (!value.is_empty()).then(|| [flag.to_string(), value.to_string()]).into_iter().flatten();
                let mut cmd = Command::new(who);
                match who {
                    "claude" => cmd.args(["-p", &prompt, "--tools", ""]).args(opt("--model", &model)).args(opt("--effort", &effort)),
                    "codex" => {
                        let level = if effort.is_empty() { String::new() } else { format!("model_reasoning_effort=\"{effort}\"") };
                        cmd.args(["exec", "--skip-git-repo-check", "-s", "read-only"]).args(opt("-m", &model)).args(opt("-c", &level)).arg(&prompt)
                    }
                    _ => cmd.arg("run").args(opt("-m", &model)).args(opt("--variant", &effort)).arg(&prompt),
                };
                cmd.current_dir(std::env::temp_dir());
                let saved: String = (0..3).map(|i| format!("{}={} {}\n", AGENTS[i], self.models[i], self.efforts[i])).collect();
                let path = self.env.home.join(MODELS_FILE);
                let _ = std::fs::create_dir_all(path.parent().unwrap()).and_then(|_| std::fs::write(&path, saved));
                self.chats.entry(key.clone()).or_default().push(("you", q));
                self.asking = Some(key.clone());
                self.question.clear();
                self.spawn(ctx, move || Msg::Answer { text: run(&mut cmd), who, skill: key });
            }
            Todo::Search => {
                let q = self.market_q.trim().to_string();
                if q.len() < 2 || self.searching {
                    return;
                }
                self.searching = true;
                self.spawn(ctx, move || Msg::Hits(search(&q)));
            }
            Todo::Install(source, skill) => {
                // Names come from the network: never let one be read as a flag.
                if source.starts_with('-') || skill.starts_with('-') {
                    self.log = format!("install: refusing odd name {source} {skill}");
                    return;
                }
                let mut cmd = Command::new("npx");
                cmd.args(["-y", "skills", "add", &source, "-s", &skill, "-g", "-y"]);
                self.job(ctx, format!("npx skills add {source} -s {skill}"), cmd);
            }
            Todo::Lookup(names) => {
                self.log = format!("Looking up {} skill name(s) on skills.sh...", names.len());
                let read = |n: &String| self.find(n).and_then(|s| std::fs::read_to_string(s.dir.as_ref()?.join("SKILL.md")).ok());
                let mine: Vec<(String, Option<String>)> = names.iter().map(|n| (n.clone(), read(n))).collect();
                // Comparing costs a GitHub request per candidate, so a bulk lookup only checks the top one.
                let depth = if mine.len() == 1 { 4 } else { 1 };
                self.spawn(ctx, move || {
                    let look = |(n, md): (String, Option<String>)| {
                        let mut hits: Vec<Hit> = search(&n).ok()?.into_iter().filter(|h| h.skill == n || h.name == n).collect();
                        hits.sort_by(|a, b| b.installs.cmp(&a.installs));
                        // The comparison happens here: only the public repo is fetched, your text is not sent.
                        for h in hits.iter_mut().take(depth) {
                            h.same = md.as_ref().and_then(|md| Some(core::similarity(md, &remote_md(&h.source, &h.skill)?)));
                        }
                        hits.sort_by(|a, b| b.same.cmp(&a.same));
                        std::thread::sleep(std::time::Duration::from_millis(150));
                        Some((n, hits))
                    };
                    Msg::Matches(mine.into_iter().filter_map(look).collect())
                });
            }
            Todo::Read(name, hit) => {
                self.preview = Some((name, hit.clone(), None));
                self.spawn(ctx, move || {
                    let text = remote_md(&hit.source, &hit.skill);
                    Msg::Text(hit, text)
                });
            }
            Todo::Track(name, source) => {
                let Some(s) = self.find(&name) else { return };
                if source.starts_with('-') || name.starts_with('-') {
                    return;
                }
                // The installer deletes a real copy in its way and ignores the others, so move
                // them all to the Trash first and link the new copy back where the skill was.
                let had: Vec<usize> = (0..s.entries.len()).filter(|i| s.entries[*i].is_some()).collect();
                if let Err(e) = core::remove(&self.env, &name, None) {
                    self.report(&format!("track {name}"), Err(e));
                    return;
                }
                self.matches.remove(&name);
                let mut cmd = Command::new("npx");
                cmd.args(["-y", "skills", "add", &source, "-s", &name, "-g", "-y"]);
                self.job_then(ctx, format!("track {name} from {source} (your old copy is in the Trash)"), cmd, Some((name, had)));
            }
            Todo::InstallUpdate => {
                if let Some((_, url)) = self.update.clone() {
                    self.updating = true;
                    self.spawn(ctx, move || Msg::Updated(update::install(&url)));
                }
            }
        }
    }

    fn receive(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.busy -= 1;
            match msg {
                Msg::Job { title, out, relink } => {
                    self.log = format!("{title}\n{out}");
                    self.rescan();
                    if let Some((name, had)) = relink {
                        if let Some(s) = self.find(&name).cloned() {
                            for i in had.into_iter().filter(|i| s.entries[*i].is_none()) {
                                let _ = core::link(&self.env, &s, i);
                            }
                            self.rescan();
                        }
                    }
                }
                Msg::Text(hit, text) => {
                    // Ignore a reply for a result the user has already closed or moved on from.
                    if let Some(p) = self.preview.as_mut().filter(|p| p.1.source == hit.source && p.1.skill == hit.skill) {
                        p.2 = Some(text.unwrap_or_default());
                    }
                }
                Msg::Matches(found) => {
                    let hit = found.iter().filter(|(_, h)| !h.is_empty()).count();
                    self.log = format!("skills.sh: {hit} of {} looked up have a skill with the same name there.", found.len());
                    self.matches.extend(found);
                }
                Msg::Usage(u) => {
                    self.log = format!("Usage scan done: {} distinct skills used in Claude Code or Codex.", u.len());
                    self.usage = Some(u);
                }
                Msg::Answer { skill, who, text } => {
                    self.chats.entry(skill).or_default().push((who, text));
                    self.asking = None;
                }
                Msg::Models(i, list) => self.model_lists[i] = list,
                Msg::Hits(r) => {
                    self.searching = false;
                    self.searched = true;
                    self.hits = r.unwrap_or_else(|e| {
                        self.log = format!("skills.sh search FAILED: {e}");
                        Vec::new()
                    });
                }
                Msg::UpdateAvailable(u) => self.update = u,
                Msg::Updated(Ok(app)) => update::relaunch(&app),
                Msg::Updated(Err(e)) => {
                    self.updating = false;
                    self.log = format!("update FAILED: {e}");
                }
            }
        }
    }

    fn nav(&mut self, ui: &mut egui::Ui, todo: &mut Vec<Todo>) {
        ui.add_space(4.0);
        ui.label(RichText::new("Skill Construct").size(20.0).strong());
        ui.label(RichText::new(concat!("Agent skills on this Mac  ·  v", env!("CARGO_PKG_VERSION"))).size(12.0).color(MUTED));
        ui.add_space(8.0);

        let count = |v: &View| self.skills.iter().chain(&self.plugin_skills).filter(|s| self.in_view(v, s)).count();
        let mut items = vec![(View::All, "All skills".to_string()), (View::Issues, "Needs attention".to_string())];
        if self.usage.is_some() {
            items.push((View::Unused, "Never used".to_string()));
        }
        items.push((View::Plugins, "Built in & plugins".to_string()));
        let mut sources: Vec<(String, usize)> = Vec::new();
        for s in &self.skills {
            match sources.iter_mut().find(|(n, _)| n == &s.source) {
                Some(e) => e.1 += 1,
                None => sources.push((s.source.clone(), 1)),
            }
        }
        sources.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let counted: Vec<(View, String, usize)> = items.into_iter().map(|(v, l)| (v.clone(), l, count(&v))).collect();

        caption(ui, "LIBRARY");
        for (view, label, n) in counted {
            if nav_item(ui, self.view == view, &label, n) {
                self.view = view;
            }
        }
        caption(ui, "DISCOVER");
        if nav_item(ui, self.view == View::Discover, "Browse skills.sh", self.hits.len()) {
            self.view = View::Discover;
        }
        caption(ui, "SOURCES");
        // Leave room at the bottom for the add box.
        let list_h = (ui.available_height() - 112.0).max(60.0);
        egui::ScrollArea::vertical().max_height(list_h).auto_shrink(false).show(ui, |ui| {
            for (src, n) in sources {
                let view = View::Source(src.clone());
                if nav_item(ui, self.view == view, &src, n) {
                    self.view = view;
                }
            }
        });
        caption(ui, "ADD A SKILL");
        let field = egui::TextEdit::singleline(&mut self.add_src)
            .hint_text("folder, owner/repo or URL")
            .desired_width(f32::INFINITY);
        let entered = ui.add(field).lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.add_sized([ui.available_width(), 28.0], egui::Button::new("Add skill")).clicked() || entered {
            todo.push(Todo::Add);
        }
    }

    fn table(&mut self, ui: &mut egui::Ui, todo: &mut Vec<Todo>) {
        let title = match &self.view {
            View::All => "All skills".to_string(),
            View::Issues => "Needs attention".to_string(),
            View::Unused => "Never used".to_string(),
            View::Plugins => "Built in & plugins".to_string(),
            View::Discover => "Browse skills.sh".to_string(),
            View::Source(s) => s.clone(),
        };
        ui.horizontal(|ui| {
            ui.heading(title);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Rescan").clicked() {
                    todo.push(Todo::Rescan);
                }
                if ui.button("Update all").on_hover_text("Runs: npx skills update -g -y").clicked() {
                    todo.push(Todo::Update(None));
                }
                if ui.button("Scan usage").on_hover_text("Counts skill use in your Claude Code and Codex transcripts").clicked() {
                    todo.push(Todo::ScanUsage);
                }
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Search name, text or source").desired_width(240.0));
            });
        });

        let needle = self.filter.to_lowercase();
        let mut rows: Vec<&Skill> = self
            .skills
            .iter()
            .chain(&self.plugin_skills)
            .filter(|s| self.in_view(&self.view, s))
            .filter(|s| [&s.name, &s.description, &s.source].iter().any(|f| f.to_lowercase().contains(&needle)))
            .collect();
        if self.sort_by_uses {
            rows.sort_by_key(|s| self.uses(&s.name).map_or(0, |u| u.0));
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} shown", rows.len())).size(12.5).color(MUTED));
            ui.label(RichText::new("   filled dot = real copy    ring = link    red = broken").size(12.0).color(MUTED));
            // Bulk removal only for a deliberately narrowed list, never the whole table by accident.
            let narrowed = !needle.is_empty() || !matches!(self.view, View::All | View::Plugins);
            let names: Vec<String> = rows.iter().filter(|s| !s.plugin).map(|s| s.name.clone()).collect();
            if !self.checked.is_empty() {
                // Ticked rows win over the "everything shown" shortcut.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Clear").clicked() {
                        self.checked.clear();
                    }
                    let text = format!("Remove {} selected", self.checked.len());
                    if ui.button(RichText::new(text).color(RED)).clicked() {
                        let mut picked: Vec<String> = self.checked.iter().cloned().collect();
                        picked.sort();
                        todo.push(Todo::ConfirmRemove(picked, None));
                    }
                });
            } else if narrowed && !names.is_empty() {
                let local: Vec<String> = rows.iter().filter(|s| !s.plugin && !s.tracked && s.outside.is_none()).map(|s| s.name.clone()).collect();
                if !local.is_empty() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let hover = format!("Sends the names of the {} untracked skills shown to skills.sh to see which exist there", local.len());
                        if ui.button("Find sources on skills.sh").on_hover_text(hover).clicked() {
                            todo.push(Todo::Lookup(local));
                        }
                    });
                }
            }
            if self.checked.is_empty() && narrowed && !names.is_empty() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let text = format!("Remove these {} everywhere", names.len());
                    if ui.button(RichText::new(text).color(RED)).clicked() {
                        todo.push(Todo::ConfirmRemove(names, None));
                    }
                });
            }
        });
        ui.add_space(4.0);

        let n = self.env.roots.len() as f32;
        let fixed = n * HARNESS_W + USES_W + SOURCE_W;
        // x offsets of the fixed columns, measured from the right edge of a row.
        let harness_x = |rect: Rect, i: usize| rect.right() - fixed + HARNESS_W * (i as f32 + 0.5);
        let uses_x = |rect: Rect| rect.right() - SOURCE_W - USES_W + 8.0;
        let source_rect = |rect: Rect| Rect::from_min_max(egui::pos2(rect.right() - SOURCE_W + 8.0, rect.top()), rect.max - vec2(10.0, 0.0));

        let (head, head_resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
        let small = FontId::proportional(11.0);
        let p = ui.painter().clone();
        p.text(head.left_center() + vec2(40.0, 0.0), Align2::LEFT_CENTER, "SKILL", small.clone(), MUTED);
        for (i, r) in self.env.roots.iter().enumerate() {
            p.text(egui::pos2(harness_x(head, i), head.center().y), Align2::CENTER_CENTER, r.label.to_uppercase(), small.clone(), MUTED);
        }
        let uses_head = if self.sort_by_uses { "USES (LEAST FIRST)" } else { "USES" };
        p.text(egui::pos2(uses_x(head), head.center().y), Align2::LEFT_CENTER, uses_head, small.clone(), if self.sort_by_uses { ACCENT } else { MUTED });
        p.text(source_rect(head).left_center(), Align2::LEFT_CENTER, "SOURCE", small, MUTED);
        p.hline(head.x_range(), head.bottom(), Stroke::new(1.0, BORDER));
        if head_resp.on_hover_text("Click to sort by least used").clicked() {
            self.sort_by_uses = !self.sort_by_uses;
        }

        let mut toggled = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if rows.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.label(RichText::new("Nothing here.").color(MUTED)));
            }
            for s in rows {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                let picked = self.selected.as_deref() == Some(&s.name);
                if picked {
                    ui.painter().rect_filled(rect, 8.0, SELECTED);
                } else if resp.contains_pointer() {
                    ui.painter().rect_filled(rect, 8.0, PANEL);
                }
                ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, Color32::from_rgb(26, 29, 34)));

                // Tick box in the left gutter; plugin skills cannot be removed, so they get none.
                let box_rect = Rect::from_center_size(rect.left_center() + vec2(20.0, 0.0), vec2(15.0, 15.0));
                let ticked = self.checked.contains(&s.name);
                if !s.plugin {
                    ui.painter().rect_filled(box_rect, 4.0, if ticked { ACCENT } else { Color32::TRANSPARENT });
                    ui.painter().rect_stroke(box_rect, 4.0, Stroke::new(1.2, if ticked { ACCENT } else { MUTED }), egui::StrokeKind::Inside);
                    if ticked {
                        let c = box_rect.center();
                        let tick = vec![c + vec2(-3.5, 0.0), c + vec2(-1.0, 2.8), c + vec2(3.8, -3.0)];
                        ui.painter().add(egui::Shape::line(tick, Stroke::new(1.8, BG)));
                    }
                }
                let name_w = (rect.width() - fixed - 50.0).max(80.0);
                let name_rect = Rect::from_min_size(rect.min + vec2(40.0, 6.0), vec2(name_w, 19.0));
                let desc_rect = Rect::from_min_size(rect.min + vec2(40.0, 25.0), vec2(name_w, 17.0));
                let mut title = RichText::new(&s.name).size(14.5).strong().color(TEXT);
                if !s.issues.is_empty() {
                    title = title.color(AMBER);
                }
                text_in(ui, name_rect, title);
                let desc = if s.issues.is_empty() { s.description.clone() } else { s.issues.join("  |  ") };
                text_in(ui, desc_rect, RichText::new(desc).size(12.0).color(MUTED));

                for (i, e) in s.entries.iter().enumerate() {
                    let kind = e.as_ref().map(|e| &e.kind);
                    status_dot(ui, egui::pos2(harness_x(rect, i), rect.center().y), kind);
                }
                let uses_pos = egui::pos2(uses_x(rect), rect.center().y);
                let font = FontId::proportional(13.0);
                match self.uses(&s.name) {
                    None => ui.painter().text(uses_pos, Align2::LEFT_CENTER, "-", font, BORDER),
                    Some((0, _)) => ui.painter().text(uses_pos, Align2::LEFT_CENTER, "never", font, AMBER),
                    Some((n, t)) => ui.painter().text(uses_pos, Align2::LEFT_CENTER, format!("{n}  ·  {}", days_ago(t)), font, TEXT),
                };
                match self.matches.get(&s.name).and_then(|m| m.first()).filter(|h| !s.tracked && h.same.is_none_or(|p| p >= 50)) {
                    Some(top) => text_in(ui, source_rect(rect), RichText::new(format!("local, on {}", top.source)).size(12.5).color(ACCENT)),
                    None => text_in(ui, source_rect(rect), RichText::new(&s.source).size(12.5).color(MUTED)),
                }

                if resp.clicked() {
                    let in_gutter = resp.interact_pointer_pos().is_some_and(|p| p.x < rect.left() + 36.0);
                    if in_gutter && !s.plugin {
                        toggled = Some(s.name.clone());
                    } else {
                        todo.push(Todo::Select(s.name.clone()));
                    }
                }
            }
        });
        if let Some(name) = toggled {
            if !self.checked.remove(&name) {
                self.checked.insert(name);
            }
        }
    }

    fn detail(&mut self, ui: &mut egui::Ui, todo: &mut Vec<Todo>) {
        let Some(s) = self.selected.as_deref().and_then(|n| self.find(n)).cloned() else { return };
        ui.add_space(4.0);
        ui.label(RichText::new(&s.name).size(21.0).strong());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&s.source).size(12.5).color(ACCENT));
            let used = match self.uses(&s.name) {
                None => "usage not scanned".to_string(),
                Some((0, _)) => "never invoked in Claude Code".to_string(),
                Some((n, t)) => format!("{n} uses, last {}", days_ago(t)),
            };
            ui.label(RichText::new(format!("·  {used}")).size(12.5).color(MUTED));
        });
        ui.add_space(2.0);
        egui::ScrollArea::vertical().id_salt("desc").max_height(110.0).show(ui, |ui| {
            ui.label(RichText::new(&s.description).size(13.5).color(Color32::from_rgb(190, 196, 203)));
        });

        if !s.issues.is_empty() {
            egui::Frame::new()
                .fill(Color32::from_rgb(52, 40, 20))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for issue in &s.issues {
                        ui.label(RichText::new(issue).color(AMBER).size(13.0));
                    }
                });
        }

        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            if let Some(dir) = &s.dir {
                if ui.button("Open folder").on_hover_text(dir.display().to_string()).clicked() {
                    todo.push(Todo::Open(dir.clone()));
                }
            }
            if s.tracked && ui.button("Update from source").clicked() {
                todo.push(Todo::Update(Some(s.name.clone())));
            }
            if !s.plugin && ui.button(RichText::new("Remove everywhere").color(RED)).clicked() {
                todo.push(Todo::ConfirmRemove(vec![s.name.clone()], None));
            }
            if !self.chat_open && ui.button("Ask an agent").clicked() {
                self.chat_open = true;
            }
        });

        if s.plugin {
            let note = match s.source.strip_prefix("built into ") {
                Some(owner) => format!("Built into {owner}, which installs and updates it itself. Read-only here so it cannot be removed by accident."),
                None => "Shipped by a Claude Code plugin. Manage it with `claude plugin`.".to_string(),
            };
            ui.label(RichText::new(note).color(MUTED).size(12.5));
        }
        if let Some(o) = &s.outside {
            let note = format!(
                "Lives outside the skill folders, at {}. Whatever put it there owns it and may put it back. Removing here only deletes the links.",
                core::tilde(&self.env, o)
            );
            ui.label(RichText::new(note).color(MUTED).size(12.5));
        }
        if !s.tracked && !s.plugin && s.outside.is_none() {
            caption(ui, "ON SKILLS.SH");
            match self.matches.get(&s.name) {
                None => {
                    ui.label(RichText::new("Not linked to a source, so it cannot update.").color(MUTED).size(12.5));
                    if ui.button("Find on skills.sh").on_hover_text("Sends this skill's name to skills.sh").clicked() {
                        todo.push(Todo::Lookup(vec![s.name.clone()]));
                    }
                }
                Some(found) if found.is_empty() => {
                    ui.label(RichText::new("No skill with this name there. It is yours or came from somewhere else.").color(MUTED).size(12.5));
                }
                Some(found) => {
                    ui.label(RichText::new("Same name, compared with your copy. Read it before switching.").color(MUTED).size(12.5));
                    for h in found.iter().take(4) {
                        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::hover());
                        let (top, bottom) = (rect.with_max_y(rect.top() + 24.0), rect.with_min_y(rect.top() + 24.0));
                        text_in(ui, top, RichText::new(format!("{}  ·  {}", h.source, compact(h.installs))).size(12.5));
                        let (verdict, color) = match h.same {
                            Some(100) => ("identical".to_string(), GREEN),
                            Some(p) if p >= 50 => (format!("{p}% same"), ACCENT),
                            Some(p) => (format!("mostly different, {p}%"), RED),
                            None => ("not compared".to_string(), MUTED),
                        };
                        text_in(ui, bottom, RichText::new(verdict).size(12.0).color(color));
                        let mut right = ui.new_child(UiBuilder::new().max_rect(bottom).layout(Layout::right_to_left(Align::Center)));
                        let hover = "Moves your copy to the Trash, installs this one, and keeps it updating";
                        if right.small_button("Switch to this").on_hover_text(hover).clicked() {
                            todo.push(Todo::Track(s.name.clone(), h.source.clone()));
                        }
                        if right.small_button("Read it").on_hover_text("Opens the full skill text in a large window").clicked() {
                            todo.push(Todo::Read(s.name.clone(), h.clone()));
                        }
                    }
                }
            }
        }
        if !s.plugin {
            caption(ui, "INSTALLED IN");
            for (i, root) in self.env.roots.iter().enumerate() {
                let entry = s.entries[i].as_ref();
                let kind = entry.map(|e| &e.kind);
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
                status_dot(ui, rect.left_center() + vec2(8.0, 0.0), kind);
                let p = ui.painter();
                p.text(rect.left_center() + vec2(24.0, 0.0), Align2::LEFT_CENTER, root.label, FontId::proportional(13.5), TEXT);
                p.text(rect.left_center() + vec2(110.0, 0.0), Align2::LEFT_CENTER, kind_word(kind), FontId::proportional(12.5), MUTED);
                let mut right = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::right_to_left(Align::Center)));
                match kind {
                    None => {
                        if s.canonical().is_some() && right.small_button("Link here").clicked() {
                            todo.push(Todo::Link(s.name.clone(), i));
                        }
                    }
                    Some(Kind::Dir) => {
                        if right.small_button("Move to Trash").clicked() {
                            todo.push(Todo::ConfirmRemove(vec![s.name.clone()], Some(i)));
                        }
                    }
                    Some(_) => {
                        if right.small_button("Unlink").clicked() {
                            todo.push(Todo::Remove(s.name.clone(), Some(i)));
                        }
                    }
                }
            }
        }

    }

    fn market(&mut self, ui: &mut egui::Ui, todo: &mut Vec<Todo>) {
        ui.horizontal(|ui| {
            ui.heading("Browse skills.sh");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let go = ui.add_enabled(!self.searching, egui::Button::new("Search")).clicked();
                let field = egui::TextEdit::singleline(&mut self.market_q).hint_text("Search the skills.sh directory").desired_width(320.0);
                if go || (ui.add(field).lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                    todo.push(Todo::Search);
                }
            });
        });
        let note = "The public directory behind `npx skills`. Install runs: npx skills add <source> -s <skill> -g -y";
        ui.label(RichText::new(note).size(12.0).color(MUTED));
        ui.add_space(4.0);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if self.hits.is_empty() {
                let hint = match (self.searching, self.searched) {
                    (true, _) => "Searching...",
                    (_, true) => "Nothing found.",
                    _ => "Search by what the skill should do: react, pdf, testing, seo.",
                };
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.label(RichText::new(hint).color(MUTED)));
            }
            for h in &self.hits {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, Color32::from_rgb(26, 29, 34)));
                let name_w = (rect.width() - 330.0).max(80.0);
                text_in(ui, Rect::from_min_size(rect.min + vec2(14.0, 6.0), vec2(name_w, 19.0)), RichText::new(&h.name).size(14.5).strong());
                text_in(ui, Rect::from_min_size(rect.min + vec2(14.0, 25.0), vec2(name_w, 17.0)), RichText::new(&h.source).size(12.0).color(MUTED));
                ui.painter().text(
                    rect.right_center() - vec2(180.0, 0.0),
                    Align2::RIGHT_CENTER,
                    format!("{} installs", compact(h.installs)),
                    FontId::proportional(12.5),
                    MUTED,
                );
                let mut right = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 0.0))).layout(Layout::right_to_left(Align::Center)));
                if self.skills.iter().any(|s| s.name == h.name) {
                    right.label(RichText::new("installed").color(GREEN).size(12.5));
                } else if right.button("Install").clicked() {
                    todo.push(Todo::Install(h.source.clone(), h.skill.clone()));
                }
                if right.small_button("Read it").on_hover_text("Opens the full skill text in a large window").clicked() {
                    todo.push(Todo::Read(String::new(), h.clone()));
                }
            }
        });
    }

    fn chat(&mut self, ui: &mut egui::Ui, todo: &mut Vec<Todo>) {
        let key = self.selected.clone().filter(|n| self.find(n).is_some()).unwrap_or_default();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Ask an agent").size(18.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Hide").clicked() {
                    self.chat_open = false;
                }
            });
        });
        let about = if key.is_empty() { "About your whole library. Select a skill to ask about just that one.".into() } else { format!("About {key}") };
        ui.label(RichText::new(about).size(12.5).color(MUTED));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for (i, a) in AGENTS.iter().enumerate() {
                ui.selectable_value(&mut self.agent, i, *a);
            }
        });
        let a = self.agent;
        let levels = self.levels(a);
        ui.horizontal(|ui| {
            let half = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
            let (list, model, effort, filter) = (&self.model_lists[a], &mut self.models[a], &mut self.efforts[a], &mut self.model_filter);
            let shown = if model.is_empty() { "Default model".to_string() } else { model.clone() };
            egui::ComboBox::from_id_salt(("model", a))
                .selected_text(shown)
                .width(half)
                .height(380.0)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show_ui(ui, |ui| {
                    // Long catalogs (opencode lists hundreds) get a filter box.
                    if list.len() > 12 {
                        ui.add(egui::TextEdit::singleline(filter).hint_text("Filter").desired_width(f32::INFINITY));
                    }
                    let needle = filter.to_lowercase();
                    let mut pick = None;
                    if ui.selectable_label(model.is_empty(), "Default model").clicked() {
                        pick = Some(String::new());
                    }
                    for (m, _) in list.iter().filter(|(m, _)| list.len() <= 12 || m.to_lowercase().contains(&needle)) {
                        if ui.selectable_label(m == model, m).clicked() {
                            pick = Some(m.clone());
                        }
                    }
                    if let Some(m) = pick {
                        *model = m;
                        effort.clear();
                        ui.close();
                    }
                });
            let shown = if effort.is_empty() { "Default thinking".to_string() } else { format!("Thinking: {effort}") };
            egui::ComboBox::from_id_salt(("effort", a)).selected_text(shown).width(half).show_ui(ui, |ui| {
                ui.selectable_value(effort, String::new(), "Default thinking");
                for l in &levels {
                    ui.selectable_value(effort, l.clone(), l);
                }
            });
        });
        ui.add_space(2.0);

        let waiting = self.asking.is_some();
        // Bottom-up so the question box stays pinned and the conversation takes whatever is left.
        ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
            let ready = !waiting && !self.question.trim().is_empty();
            let button = egui::Button::new(format!("Ask {}", AGENTS[a])).min_size(vec2(ui.available_width(), 30.0));
            let send = ui.add_enabled(ready, button).clicked();
            let field = ui.add(
                egui::TextEdit::multiline(&mut self.question)
                    .hint_text("Do I still need this? Does it overlap with another skill?  (Enter sends)")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .return_key(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter)),
            );
            let entered = field.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
            if ready && (send || entered) {
                todo.push(Todo::Ask(key.clone()));
            }
            ui.add_space(4.0);
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                egui::ScrollArea::vertical().id_salt("chat").auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
                    let msgs = self.chats.get(&key).map(Vec::as_slice).unwrap_or_default();
                    if msgs.is_empty() && !waiting {
                        ui.add_space(30.0);
                        ui.vertical_centered(|ui| ui.label(RichText::new("Ask whether to keep, merge or remove.\nThe agent sees the skill and the rest of your library.").color(MUTED).size(12.5)));
                    }
                    for (who, text) in msgs {
                        let mine = *who == "you";
                        if !mine {
                            ui.label(RichText::new(*who).size(11.5).color(MUTED));
                        }
                        // Your messages sit to the right, the agent's to the left.
                        let gap = if mine { Margin { left: 48, ..Margin::ZERO } } else { Margin { right: 20, ..Margin::ZERO } };
                        egui::Frame::new()
                            .fill(if mine { SELECTED } else { CARD })
                            .corner_radius(CornerRadius::same(11))
                            .inner_margin(Margin::symmetric(12, 9))
                            .outer_margin(gap)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.add(Label::new(RichText::new(text).size(14.0)).wrap().selectable(true));
                            });
                        ui.add_space(6.0);
                    }
                    if waiting {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new(format!("{} is thinking...", AGENTS[a])).color(MUTED).size(12.5));
                        });
                    }
                });
            });
        });
    }

    fn status(&mut self, ui: &mut egui::Ui, todo: &mut Vec<Todo>) {
        ui.horizontal(|ui| {
            if self.busy > 0 {
                ui.spinner();
            }
            if self.updating {
                ui.label(RichText::new("Downloading update...").size(12.5).color(ACCENT));
            } else if let Some((v, _)) = &self.update {
                if ui.small_button(RichText::new(format!("Update to {v} and restart")).color(ACCENT)).clicked() {
                    todo.push(Todo::InstallUpdate);
                }
            }
            let last = self.log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
            // CLI output leads with box-drawing glyphs the UI font cannot draw.
            let last = last.trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button(if self.log_open { "Hide log" } else { "Show log" }).clicked() {
                    self.log_open = !self.log_open;
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(Label::new(RichText::new(last).size(12.5).color(MUTED)).truncate());
                });
            });
        });
        if self.log_open {
            egui::ScrollArea::vertical().id_salt("log").max_height(200.0).stick_to_bottom(true).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(Label::new(RichText::new(&self.log).monospace()).wrap().selectable(true));
            });
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.receive();
        let mut todo = Vec::new();
        let side = egui::Frame::new().fill(PANEL).inner_margin(Margin::same(14));

        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(14, 7)))
            .show(ui, |ui| self.status(ui, &mut todo));
        egui::Panel::left("nav").frame(side).resizable(false).default_size(220.0).show(ui, |ui| self.nav(ui, &mut todo));
        // Added first so it sits outermost: skills | details | chat.
        if self.chat_open {
            egui::Panel::right("chat").frame(side).resizable(true).default_size(340.0).show(ui, |ui| self.chat(ui, &mut todo));
        }
        // No selection, no panel: the list gets the room instead.
        if self.selected.as_deref().is_some_and(|n| self.find(n).is_some()) {
            egui::Panel::right("detail").frame(side).resizable(true).default_size(330.0).show(ui, |ui| self.detail(ui, &mut todo));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::same(16)))
            .show(ui, |ui| match self.view {
                View::Discover => self.market(ui, &mut todo),
                _ => self.table(ui, &mut todo),
            });

        if let Some((name, hit, text)) = self.preview.clone() {
            let mut close = false;
            let size = ctx.content_rect().size() * 0.8;
            egui::Window::new("Read skill")
                .collapsible(false)
                .resizable(false)
                .title_bar(false)
                .fixed_size(size)
                .frame(egui::Frame::window(&ctx.global_style()).inner_margin(Margin::same(18)))
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&hit.name).size(18.0).strong());
                        ui.label(RichText::new(format!("{}  ·  {} installs", hit.source, compact(hit.installs))).color(MUTED).size(12.5));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            close = ui.button("Close").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
                            let (label, action) = match name.is_empty() {
                                true => ("Install", Todo::Install(hit.source.clone(), hit.skill.clone())),
                                false => ("Switch to this", Todo::Track(name.clone(), hit.source.clone())),
                            };
                            if ui.button(RichText::new(label).color(ACCENT)).clicked() {
                                todo.push(action);
                                close = true;
                            }
                            if ui.button("Open on skills.sh").clicked() {
                                todo.push(Todo::Open(PathBuf::from(format!("https://skills.sh/{}/{}", hit.source, hit.skill))));
                            }
                        });
                    });
                    ui.separator();
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match text.as_deref() {
                        None => drop(ui.label(RichText::new("Loading the skill text...").color(MUTED))),
                        Some("") => drop(ui.label(RichText::new("Could not load the text from GitHub. Open it on skills.sh instead.").color(MUTED))),
                        Some(t) => drop(ui.add(Label::new(RichText::new(t).monospace().size(13.0)).wrap())),
                    });
                });
            if close {
                self.preview = None;
            }
        }

        if let Some((names, only)) = self.confirm.clone() {
            let what = match names.as_slice() {
                [one] => format!("'{one}'"),
                many => format!("{} skills", many.len()),
            };
            let scope = only.map_or("every harness".to_string(), |i| format!("{} only", self.env.roots[i].label));
            egui::Window::new("Confirm removal")
                .collapsible(false)
                .resizable(false)
                .title_bar(false)
                .max_width(420.0)
                .frame(egui::Frame::window(&ctx.global_style()).inner_margin(Margin::same(18)))
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(RichText::new(format!("Remove {what} from {scope}?")).size(16.0).strong());
                    if names.len() > 1 {
                        ui.label(RichText::new(names.join(", ")).size(13.0));
                    }
                    ui.label(
                        RichText::new("Real folders go to the Trash. Links that pointed at a trashed copy are removed too.")
                            .color(MUTED)
                            .size(12.5),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Remove").color(RED)).clicked() {
                            todo.extend(names.iter().map(|n| Todo::Remove(n.clone(), only)));
                            self.confirm = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm = None;
                        }
                    });
                });
        }

        for t in todo {
            self.apply(ctx, t);
        }
    }
}
