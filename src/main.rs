mod core;

use core::{Env, Kind, Skill, Usage};
use eframe::egui::{
    self, vec2, Align, Align2, Color32, CornerRadius, FontId, Label, Layout, Margin, Rect, RichText, Sense, Stroke,
    UiBuilder,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::SystemTime;

const AGENTS: [&str; 3] = ["claude", "codex", "opencode"];

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
        viewport: egui::ViewportBuilder::default().with_inner_size([1520.0, 940.0]).with_min_inner_size([1100.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Skill Construct",
        options,
        Box::new(|cc| {
            setup_look(&cc.egui_ctx);
            Ok(Box::new(App::new()))
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
    Job { title: String, out: String },
    Usage(Usage),
    Answer { skill: String, who: &'static str, text: String },
}

#[derive(Clone, PartialEq)]
enum View {
    All,
    Issues,
    Unused,
    Plugins,
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
}

struct App {
    env: Env,
    skills: Vec<Skill>,
    plugin_skills: Vec<Skill>,
    usage: Option<Usage>,
    selected: Option<String>,
    view: View,
    filter: String,
    sort_by_uses: bool,
    add_src: String,
    confirm: Option<(Vec<String>, Option<usize>)>,
    agent: usize,
    question: String,
    /// Per skill: (who said it, text).
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
    fn new() -> App {
        let env = Env::real();
        let (tx, rx) = channel();
        let mut app = App {
            skills: Vec::new(),
            plugin_skills: Vec::new(),
            usage: None,
            selected: None,
            view: View::All,
            filter: String::new(),
            sort_by_uses: false,
            add_src: String::new(),
            confirm: None,
            agent: 0,
            question: "Do I still need this skill? Does it overlap with another one I have?".into(),
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
        app
    }

    fn rescan(&mut self) {
        self.skills = core::scan(&self.env);
        self.plugin_skills = core::plugin_skills(&self.env);
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

    fn job(&mut self, ctx: &egui::Context, title: String, mut cmd: Command) {
        self.log = format!("{title}\nrunning...");
        self.spawn(ctx, move || Msg::Job { out: run(&mut cmd), title });
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

    fn in_view(&self, s: &Skill) -> bool {
        match &self.view {
            View::Plugins => s.plugin,
            _ if s.plugin => false,
            View::All => true,
            View::Issues => !s.issues.is_empty(),
            View::Unused => matches!(self.uses(&s.name), Some((0, _))),
            View::Source(src) => &s.source == src,
        }
    }

    fn prompt(&self, skill: &Skill, question: &str) -> String {
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
        let others: String = self
            .skills
            .iter()
            .chain(&self.plugin_skills)
            .filter(|s| s.name != skill.name)
            .map(|s| format!("- {}: {}\n", s.name, s.description.chars().take(200).collect::<String>()))
            .collect();
        let so_far: String = self
            .chats
            .get(&skill.name)
            .map(|c| c.iter().map(|(who, text)| format!("{who}: {text}\n")).collect())
            .unwrap_or_else(|| "(none)".into());
        format!(
            "You are helping me audit the agent skills installed on my computer. Answer directly and \
             briefly, with a clear keep / remove / merge recommendation when asked. Plain text only, no \
             markdown. Do not use tools or change anything.\n\n\
             Skill under review: {}\nSource: {}\nInstalled for: {}\nUsage: {usage}\nKnown issues: {}\n\n\
             === SKILL.md ===\n{}\n\n=== My other skills ===\n{others}\n=== Conversation so far ===\n{so_far}\n\nMy question: {question}",
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
                let dir = self.env.home.join(".claude/projects");
                self.log = "Scanning Claude Code transcripts for skill usage...".into();
                self.spawn(ctx, move || {
                    let mut u = Usage::new();
                    core::scan_usage(&dir, &mut u);
                    Msg::Usage(u)
                });
            }
            Todo::Ask(name) => {
                let Some(skill) = self.find(&name) else { return };
                let q = self.question.trim().to_string();
                let prompt = self.prompt(skill, &q);
                let who = AGENTS[self.agent];
                let mut cmd = Command::new(who);
                match who {
                    "claude" => cmd.args(["-p", &prompt, "--tools", ""]),
                    "codex" => cmd.args(["exec", "--skip-git-repo-check", "-s", "read-only", &prompt]),
                    _ => cmd.args(["run", &prompt]),
                };
                cmd.current_dir(std::env::temp_dir());
                self.chats.entry(name.clone()).or_default().push(("you", q));
                self.asking = Some(name.clone());
                self.question.clear();
                self.spawn(ctx, move || Msg::Answer { text: run(&mut cmd), who, skill: name });
            }
        }
    }

    fn receive(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.busy -= 1;
            match msg {
                Msg::Job { title, out } => {
                    self.log = format!("{title}\n{out}");
                    self.rescan();
                }
                Msg::Usage(u) => {
                    self.log = format!("Usage scan done: {} distinct skills invoked in Claude Code.", u.len());
                    self.usage = Some(u);
                }
                Msg::Answer { skill, who, text } => {
                    self.chats.entry(skill).or_default().push((who, text));
                    self.asking = None;
                }
            }
        }
    }

    fn nav(&mut self, ui: &mut egui::Ui, todo: &mut Vec<Todo>) {
        ui.add_space(4.0);
        ui.label(RichText::new("Skill Construct").size(20.0).strong());
        ui.label(RichText::new("Agent skills on this Mac").size(12.0).color(MUTED));
        ui.add_space(8.0);

        let count = |v: View| {
            let probe = App { view: v, ..self.shallow() };
            self.skills.iter().chain(&self.plugin_skills).filter(|s| probe.in_view(s)).count()
        };
        let mut items = vec![(View::All, "All skills".to_string()), (View::Issues, "Needs attention".to_string())];
        if self.usage.is_some() {
            items.push((View::Unused, "Never used".to_string()));
        }
        items.push((View::Plugins, "From plugins".to_string()));
        let mut sources: Vec<(String, usize)> = Vec::new();
        for s in &self.skills {
            match sources.iter_mut().find(|(n, _)| n == &s.source) {
                Some(e) => e.1 += 1,
                None => sources.push((s.source.clone(), 1)),
            }
        }
        sources.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let counted: Vec<(View, String, usize)> = items.into_iter().map(|(v, l)| (v.clone(), l, count(v))).collect();

        caption(ui, "LIBRARY");
        for (view, label, n) in counted {
            if nav_item(ui, self.view == view, &label, n) {
                self.view = view;
            }
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

    /// Just enough of self to reuse `in_view` with a different view while counting.
    fn shallow(&self) -> App {
        let (tx, rx) = channel();
        App {
            env: Env::at(self.env.home.clone()),
            skills: Vec::new(),
            plugin_skills: Vec::new(),
            usage: self.usage.clone(),
            selected: None,
            view: View::All,
            filter: String::new(),
            sort_by_uses: false,
            add_src: String::new(),
            confirm: None,
            agent: 0,
            question: String::new(),
            chats: HashMap::new(),
            asking: None,
            log: String::new(),
            log_open: false,
            busy: 0,
            tx,
            rx,
        }
    }

    fn table(&mut self, ui: &mut egui::Ui, todo: &mut Vec<Todo>) {
        let title = match &self.view {
            View::All => "All skills".to_string(),
            View::Issues => "Needs attention".to_string(),
            View::Unused => "Never used".to_string(),
            View::Plugins => "From plugins".to_string(),
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
                if ui.button("Scan usage").on_hover_text("Counts skill calls in your Claude Code transcripts").clicked() {
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
            .filter(|s| self.in_view(s))
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
            if narrowed && !names.is_empty() {
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
        p.text(head.left_center() + vec2(14.0, 0.0), Align2::LEFT_CENTER, "SKILL", small.clone(), MUTED);
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

                let name_w = (rect.width() - fixed - 24.0).max(80.0);
                let name_rect = Rect::from_min_size(rect.min + vec2(14.0, 6.0), vec2(name_w, 19.0));
                let desc_rect = Rect::from_min_size(rect.min + vec2(14.0, 25.0), vec2(name_w, 17.0));
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
                text_in(ui, source_rect(rect), RichText::new(&s.source).size(12.5).color(MUTED));

                if resp.clicked() {
                    todo.push(Todo::Select(s.name.clone()));
                }
            }
        });
    }

    fn detail(&mut self, ui: &mut egui::Ui, todo: &mut Vec<Todo>) {
        let Some(s) = self.selected.as_deref().and_then(|n| self.find(n)).cloned() else {
            ui.add_space(80.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No skill selected").size(16.0).color(MUTED));
                ui.label(RichText::new("Pick one to see where it lives, fix it,\nor ask an agent whether you need it.").color(MUTED).size(12.5));
            });
            return;
        };
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
        ui.horizontal(|ui| {
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
        });

        if s.plugin {
            ui.label(RichText::new("Shipped by a Claude Code plugin. Manage it with `claude plugin`.").color(MUTED).size(12.5));
        } else {
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

        caption(ui, "ASK AN AGENT");
        ui.horizontal(|ui| {
            for (i, a) in AGENTS.iter().enumerate() {
                ui.selectable_value(&mut self.agent, i, *a);
            }
        });
        let waiting = self.asking.is_some();
        let chat_h = (ui.available_height() - 104.0).max(60.0);
        egui::ScrollArea::vertical().id_salt("chat").max_height(chat_h).auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
            for (who, text) in self.chats.get(&s.name).into_iter().flatten() {
                let mine = *who == "you";
                if !mine {
                    ui.label(RichText::new(*who).size(11.5).color(MUTED));
                }
                egui::Frame::new()
                    .fill(if mine { SELECTED } else { CARD })
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(Label::new(RichText::new(text).size(13.5)).wrap().selectable(true));
                    });
                ui.add_space(4.0);
            }
            if waiting {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(format!("{} is thinking...", AGENTS[self.agent])).color(MUTED).size(12.5));
                });
            }
        });
        ui.add(
            egui::TextEdit::multiline(&mut self.question)
                .hint_text("Ask about this skill")
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        let ready = !waiting && !self.question.trim().is_empty();
        if ui.add_enabled(ready, egui::Button::new(format!("Ask {}", AGENTS[self.agent]))).clicked() {
            todo.push(Todo::Ask(s.name.clone()));
        }
    }

    fn status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.busy > 0 {
                ui.spinner();
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
            .show(ui, |ui| self.status(ui));
        egui::Panel::left("nav").frame(side).resizable(false).default_size(220.0).show(ui, |ui| self.nav(ui, &mut todo));
        egui::Panel::right("detail").frame(side).resizable(true).default_size(400.0).show(ui, |ui| self.detail(ui, &mut todo));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::same(16)))
            .show(ui, |ui| self.table(ui, &mut todo));

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
