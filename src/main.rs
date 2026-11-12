mod core;

use core::{Env, Kind, Skill, Usage};
use eframe::egui::{self, Color32, RichText};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::SystemTime;

const AGENTS: [&str; 3] = ["claude", "codex", "opencode"];
const RED: Color32 = Color32::from_rgb(230, 90, 90);
const AMBER: Color32 = Color32::from_rgb(230, 170, 70);
const GREEN: Color32 = Color32::from_rgb(110, 200, 130);
const BLUE: Color32 = Color32::from_rgb(120, 170, 240);

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1360.0, 860.0]),
        ..Default::default()
    };
    eframe::run_native("Skillet", options, Box::new(|_cc| Ok(Box::new(App::new()))))
}

enum Msg {
    Job { title: String, out: String },
    Usage(Usage),
    Answer { skill: String, text: String },
}

/// Things the UI asked for this frame; applied after drawing so the draw code only reads state.
enum Todo {
    Select(String),
    Link(String, usize),
    ConfirmRemove(String, Option<usize>),
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
    filter: String,
    issues_only: bool,
    show_plugins: bool,
    sort_by_uses: bool,
    add_src: String,
    confirm: Option<(String, Option<usize>)>,
    agent: usize,
    question: String,
    chats: HashMap<String, String>,
    asking: Option<String>,
    log: String,
    busy: usize,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

/// ponytail: commands inherit this process's PATH, so launch from a terminal (`cargo run`).
/// Wrap in a login shell if this ever ships as a double-clickable .app.
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
        Ok(0) => "today".into(),
        Ok(d) => format!("{d}d ago"),
        Err(_) => "today".into(),
    }
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
            filter: String::new(),
            issues_only: false,
            show_plugins: false,
            sort_by_uses: false,
            add_src: String::new(),
            confirm: None,
            agent: 0,
            question: "Do I still need this skill? Does it overlap with another one I have?".into(),
            chats: HashMap::new(),
            asking: None,
            log: String::new(),
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
        format!(
            "You are helping me audit the agent skills installed on my computer. Answer directly and \
             briefly, with a clear keep / remove / merge recommendation when asked. Do not use tools or change anything.\n\n\
             Skill under review: {}\nSource: {}\nInstalled for: {}\nUsage: {usage}\nKnown issues: {}\n\n\
             === SKILL.md ===\n{}\n\n=== My other skills ===\n{others}\n=== Conversation so far ===\n{}\n\nMy question: {question}",
            skill.name,
            skill.source,
            if installed.is_empty() { "plugin only".to_string() } else { installed.join(", ") },
            if skill.issues.is_empty() { "none".to_string() } else { skill.issues.join("; ") },
            md.chars().take(8000).collect::<String>(),
            self.chats.get(&skill.name).map(String::as_str).unwrap_or("(none)"),
        )
    }

    fn apply(&mut self, ctx: &egui::Context, todo: Todo) {
        match todo {
            Todo::Select(name) => self.selected = Some(name),
            Todo::Rescan => self.rescan(),
            Todo::ConfirmRemove(name, only) => self.confirm = Some((name, only)),
            Todo::Open(dir) => {
                let _ = Command::new("open").arg(dir).spawn();
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
                self.log = "scanning Claude Code transcripts for skill usage...".into();
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
                let mut cmd = Command::new(AGENTS[self.agent]);
                match AGENTS[self.agent] {
                    "claude" => cmd.args(["-p", &prompt, "--tools", ""]),
                    "codex" => cmd.args(["exec", "--skip-git-repo-check", "-s", "read-only", &prompt]),
                    _ => cmd.args(["run", &prompt]),
                };
                cmd.current_dir(std::env::temp_dir());
                let chat = self.chats.entry(name.clone()).or_default();
                chat.push_str(&format!("You: {q}\n\n"));
                self.asking = Some(name.clone());
                self.question.clear();
                let who = AGENTS[self.agent];
                self.spawn(ctx, move || Msg::Answer { text: format!("{who}: {}\n\n", run(&mut cmd)), skill: name });
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
                    self.log = format!("usage scan done: {} distinct skills invoked in Claude Code", u.len());
                    self.usage = Some(u);
                }
                Msg::Answer { skill, text } => {
                    self.chats.entry(skill).or_default().push_str(&text);
                    self.asking = None;
                }
            }
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, todo: &mut Vec<Todo>) {
        let flagged = self.skills.iter().filter(|s| !s.issues.is_empty()).count();
        ui.horizontal_wrapped(|ui| {
            ui.heading("Skillet");
            ui.label(format!(
                "{} skills, {} with issues, {} from plugins",
                self.skills.len(),
                flagged,
                self.plugin_skills.len()
            ));
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("filter").desired_width(160.0));
            ui.checkbox(&mut self.issues_only, "issues only");
            ui.checkbox(&mut self.show_plugins, "plugin skills");
            ui.checkbox(&mut self.sort_by_uses, "least used first");
            ui.separator();
            if ui.button("Rescan").clicked() {
                todo.push(Todo::Rescan);
            }
            if ui.button("Scan usage").on_hover_text("Count Skill calls in ~/.claude/projects transcripts").clicked() {
                todo.push(Todo::ScanUsage);
            }
            if ui.button("Update all").on_hover_text("npx skills update -g -y").clicked() {
                todo.push(Todo::Update(None));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Add skill:");
            ui.add(
                egui::TextEdit::singleline(&mut self.add_src)
                    .hint_text("local folder with SKILL.md, or owner/repo, or GitHub URL")
                    .desired_width(460.0),
            );
            if ui.button("Add").clicked() {
                todo.push(Todo::Add);
            }
        });
    }

    fn table(&self, ui: &mut egui::Ui, todo: &mut Vec<Todo>) {
        let needle = self.filter.to_lowercase();
        let mut rows: Vec<&Skill> = self
            .skills
            .iter()
            .chain(self.plugin_skills.iter().filter(|_| self.show_plugins))
            .filter(|s| !self.issues_only || !s.issues.is_empty())
            .filter(|s| s.name.to_lowercase().contains(&needle) || s.description.to_lowercase().contains(&needle))
            .collect();
        if self.sort_by_uses {
            rows.sort_by_key(|s| self.uses(&s.name).map_or(0, |u| u.0));
        }
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("skills").striped(true).spacing([14.0, 5.0]).show(ui, |ui| {
                ui.strong("Skill");
                for r in &self.env.roots {
                    ui.strong(r.label);
                }
                ui.strong("Uses");
                ui.strong("Source");
                ui.strong("Issues");
                ui.end_row();
                for s in rows {
                    let picked = self.selected.as_deref() == Some(&s.name);
                    if ui.selectable_label(picked, &s.name).on_hover_text(&s.description).clicked() {
                        todo.push(Todo::Select(s.name.clone()));
                    }
                    for e in &s.entries {
                        match e.as_ref().map(|e| &e.kind) {
                            Some(Kind::Dir) => ui.colored_label(GREEN, "copy"),
                            Some(Kind::Link) => ui.colored_label(BLUE, "link"),
                            Some(Kind::Broken) => ui.colored_label(RED, "BROKEN"),
                            None => ui.weak("-"),
                        };
                    }
                    match self.uses(&s.name) {
                        None => ui.weak("?"),
                        Some((0, _)) => ui.colored_label(AMBER, "0"),
                        Some((n, t)) => ui.label(format!("{n}  ({})", days_ago(t))),
                    };
                    ui.weak(&s.source);
                    if s.issues.is_empty() {
                        ui.label("");
                    } else {
                        ui.colored_label(AMBER, s.issues.len().to_string()).on_hover_text(s.issues.join("\n"));
                    }
                    ui.end_row();
                }
            });
        });
    }

    fn detail(&mut self, ui: &mut egui::Ui, todo: &mut Vec<Todo>) {
        let Some(s) = self.selected.as_deref().and_then(|n| self.find(n)).cloned() else {
            ui.weak("Select a skill to inspect it, fix where it is installed, or ask an agent about it.");
            return;
        };
        ui.heading(&s.name);
        ui.label(&s.description);
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.weak(format!("source: {}", s.source));
            if let Some((n, t)) = self.uses(&s.name) {
                ui.weak(if n == 0 { "| never invoked in Claude Code".into() } else { format!("| {n} uses, last {}", days_ago(t)) });
            }
        });
        for issue in &s.issues {
            ui.colored_label(AMBER, format!("! {issue}"));
        }
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
                todo.push(Todo::ConfirmRemove(s.name.clone(), None));
            }
        });

        if s.plugin {
            ui.weak("Shipped by a Claude Code plugin. Manage it with `claude plugin`.");
        } else {
            ui.separator();
            egui::Grid::new("where").spacing([12.0, 4.0]).show(ui, |ui| {
                for (i, root) in self.env.roots.iter().enumerate() {
                    ui.label(root.label);
                    match &s.entries[i] {
                        None => {
                            ui.weak("not installed");
                            if s.canonical().is_some() && ui.small_button("Link here").clicked() {
                                todo.push(Todo::Link(s.name.clone(), i));
                            }
                        }
                        Some(e) => {
                            let target = e.real.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                            let (text, color, verb) = match e.kind {
                                Kind::Dir => ("real copy", GREEN, "Trash copy"),
                                Kind::Link => ("link", BLUE, "Unlink"),
                                Kind::Broken => ("broken link", RED, "Unlink"),
                            };
                            ui.colored_label(color, text).on_hover_text(target);
                            if ui.small_button(verb).clicked() {
                                todo.push(match e.kind {
                                    Kind::Dir => Todo::ConfirmRemove(s.name.clone(), Some(i)),
                                    _ => Todo::Remove(s.name.clone(), Some(i)),
                                });
                            }
                        }
                    }
                    ui.end_row();
                }
            });
        }

        ui.separator();
        ui.strong("Ask an agent");
        ui.horizontal(|ui| {
            for (i, a) in AGENTS.iter().enumerate() {
                ui.radio_value(&mut self.agent, i, *a);
            }
        });
        ui.add(egui::TextEdit::multiline(&mut self.question).desired_rows(2).desired_width(f32::INFINITY));
        let waiting = self.asking.is_some();
        ui.horizontal(|ui| {
            let ready = !waiting && !self.question.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Ask")).clicked() {
                todo.push(Todo::Ask(s.name.clone()));
            }
            if waiting {
                ui.spinner();
                ui.weak("thinking...");
            }
        });
        egui::ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
            if let Some(chat) = self.chats.get(&s.name) {
                ui.add(egui::Label::new(chat.as_str()).wrap().selectable(true));
            }
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.receive();
        let mut todo = Vec::new();

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui, &mut todo);
            ui.add_space(4.0);
        });
        egui::Panel::bottom("log").resizable(true).default_size(110.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Log");
                if self.busy > 0 {
                    ui.spinner();
                }
            });
            egui::ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(&self.log).monospace()).wrap().selectable(true));
            });
        });
        egui::Panel::right("detail").resizable(true).default_size(460.0).show(ui, |ui| {
            self.detail(ui, &mut todo);
        });
        egui::CentralPanel::default().show(ui, |ui| self.table(ui, &mut todo));

        if let Some((name, only)) = self.confirm.clone() {
            let scope = only.map_or("every harness".to_string(), |i| format!("{} only", self.env.roots[i].label));
            egui::Window::new("Confirm removal")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!("Remove '{name}' from {scope}?"));
                    ui.weak("Real folders go to the Trash. Links that pointed at a trashed copy are removed too.");
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Remove").color(RED)).clicked() {
                            todo.push(Todo::Remove(name.clone(), only));
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
