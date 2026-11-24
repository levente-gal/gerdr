//! Pixel-identity checks: every chrome surface painted from a fixed view,
//! at scale 1 and 2 in both themes, hashed. The hashes change only when
//! the drawing does; a change made on purpose records the new ones (run
//! the test and copy the table it prints).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Instant;

use super::{
    AgentLine, AgentView, BarButton, BarButtons, BarStatus, FormField, FormView, Help, HelpHit,
    HelpPress, InboxButton, InboxDetailView, InboxIcon, InboxLine, InboxRow, InboxTone, InboxView,
    ItemIcon, MenuEntry, MenuHit, MenuRow, MenuView, Painter, PaneBarView, PlaceholderButton,
    PlaceholderView, ProjectMenuView, ProjectRow, QuitHit, QuitRow, QuitView, Rect, RowState,
    SidebarState, SidebarView, SwitcherRow, SwitcherView, TabStripView, TabView, Theme, ThemeMode,
    ToastView, TooltipView, WelcomeHit, WelcomeView, WorkspaceView,
};
use super::{CardTone, FeatureCard, NodeMark};
use crate::feature::{Stage, page::Stat};
use crate::{
    Action, WorkspaceKind, git::GitStatus, herdr::AgentStatus, text_field::Line, toast::Level,
};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;

/// FNV-1a over the buffer's bytes.
fn hash(mut buffer: MemoryRenderBuffer) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let _ = buffer.render().draw(|mem| {
        for byte in mem.iter() {
            h ^= *byte as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        Ok::<_, ()>(Vec::new())
    });
    h
}

fn workspaces() -> Vec<WorkspaceView> {
    vec![
        WorkspaceView {
            name: "gerdr".into(),
            kind: WorkspaceKind::Project,
            status: Some(AgentStatus::Blocked),
            branch: Some("main".into()),
            git: Some(GitStatus {
                branch: Some("main".into()),
                dirty: true,
                ahead: 2,
                behind: 1,
                worktrees: Vec::new(),
                here: None,
                root: None,
                ..GitStatus::default()
            }),
            place: "gerdr, gerdr/src".into(),
            activity: "2 tabs · claude working, zsh".into(),
            icon: None,
            more_agents: Vec::new(),
            ci: None,
            folded: false,
            restart: false,
            picks: Vec::new(),
            errors: 0,
            feature: None,
        },
        WorkspaceView {
            name: "~".into(),
            kind: WorkspaceKind::Terminal,
            status: None,
            branch: None,
            git: None,
            place: String::new(),
            activity: String::new(),
            icon: None,
            more_agents: Vec::new(),
            ci: None,
            folded: false,
            restart: false,
            picks: Vec::new(),
            errors: 0,
            feature: None,
        },
    ]
}

fn agents() -> Vec<Vec<AgentView>> {
    vec![
        vec![AgentView {
            key: crate::agents::AgentKey::Herdr("p1".into()),
            dir: None,
            status: AgentStatus::Blocked,
            kind: "claude".into(),
            name: Some("reviewer".into()),
            model: None,
            lines: vec![
                AgentLine::Title("Reviewing the split".into()),
                AgentLine::Field("Folder".into(), "gerdr/src".into()),
                AgentLine::Field("Tokens".into(), "12k".into()),
            ],
            closable: false,
            subagents: Vec::new(),
            inbox: false,
            test: None,
            icon: crate::agent_icons::AgentIcon::Claude,
            paused: false,
        }],
        vec![],
    ]
}

/// An agent of each kind and state, to look at their badges.
fn many_agents() -> Vec<Vec<AgentView>> {
    use crate::agent_icons::AgentIcon::*;
    use crate::ci::CiState;
    let base = agents().remove(0).remove(0);
    let card = |icon, kind: &str, status, test| AgentView {
        key: crate::agents::AgentKey::Herdr(kind.into()),
        kind: kind.into(),
        name: None,
        status,
        test,
        icon,
        ..base.clone()
    };
    let rows = [
        (Claude, "claude", AgentStatus::Working, None),
        (Codex, "codex", AgentStatus::Done, None),
        (Gemini, "gemini", AgentStatus::Idle, None),
        (Cursor, "cursor", AgentStatus::Working, None),
        (OpenCode, "opencode", AgentStatus::Blocked, None),
        (Copilot, "copilot", AgentStatus::Idle, None),
        (Amp, "amp", AgentStatus::Done, None),
        (Qwen, "qwen", AgentStatus::Idle, None),
        (Kimi, "kimi", AgentStatus::Idle, None),
        (Grok, "grok", AgentStatus::Idle, None),
        (Kiro, "kiro", AgentStatus::Idle, None),
        (Cline, "cline", AgentStatus::Idle, None),
        (Devin, "devin", AgentStatus::Idle, None),
        (Antigravity, "agy", AgentStatus::Idle, None),
        (Mastra, "mastracode", AgentStatus::Idle, None),
        (Generic, "droid", AgentStatus::Unknown, None),
        (
            Test,
            "cargo test",
            AgentStatus::Working,
            Some(CiState::Running),
        ),
        (Test, "npm test", AgentStatus::Done, Some(CiState::Passed)),
        (Test, "pytest", AgentStatus::Done, Some(CiState::Failed)),
    ];
    let cards = rows
        .into_iter()
        .map(|(icon, kind, status, test)| card(icon, kind, status, test))
        .collect();
    vec![cards, vec![]]
}

/// The tab strip of `tabs`, the second shown, the prefix armed, and
/// back to go to but nothing forward.
fn strip_view() -> TabStripView {
    let mut view = TabStripView::plain(tabs(), 1, true, ThemeMode::System);
    view.nav = (true, false);
    view
}

fn tabs() -> Vec<TabView> {
    vec![
        TabView {
            title: "gerdr".into(),
            status: Some(AgentStatus::Working),
            branch: Some("main".into()),
            place: "gerdr".into(),
            activity: "claude working · cargo build".into(),
            restart: Some(true),
            github: false,
            agent: Some(crate::agent_icons::AgentIcon::Claude),
            zoomed: false,
            pinned: false,
            stage: None,
            running: false,
        },
        TabView {
            title: "zsh".into(),
            activity: "cargo watch".into(),
            restart: Some(false),
            github: false,
            ..Default::default()
        },
        TabView::default(),
    ]
}

/// A terminal's bar, its command exited, restart hovered.
fn pane_bar() -> PaneBarView {
    let pane = Rect::new((0, 0).into(), (700, 400).into());
    PaneBarView {
        rect: super::pane_bar_rects(pane).0,
        label: "npm run dev -- --port 3000".into(),
        note: Some("exited"),
        status: BarStatus::Exited,
        can_restart: true,
        can_stop: false,
        can_back: false,
        can_forward: false,
        buttons: BarButtons::Terminal,
        zoom: None,
        focused: true,
        hover: Some(BarButton::Restart),
    }
}

fn menu() -> MenuView {
    MenuView {
        anchor: Rect::new((150, 0).into(), (28, 34).into()),
        rows: vec![MenuRow {
            label: "Open folder…".into(),
            hint: None,
            enabled: true,
            note: false,
            danger: false,
        }],
        hover: Some(0),
    }
}

/// A right-click menu: key hints, a disabled row, the hover elsewhere.
fn full_menu() -> MenuView {
    let row = |label: &str, hint: Option<&str>, enabled| MenuRow {
        label: label.into(),
        hint: hint.map(str::to_owned),
        enabled,
        note: false,
        danger: false,
    };
    MenuView {
        anchor: Rect::new((40, 60).into(), (160, 32).into()),
        rows: vec![
            row("New tab", Some("Ctrl+B c"), true),
            row("Rename…", None, true),
            row("Copy path", None, false),
            row("Close workspace", Some("Ctrl+B Shift+X"), true),
        ],
        hover: Some(3),
    }
}

fn toast(action: Option<&str>, level: Level) -> ToastView {
    ToastView {
        id: 7,
        text: "Zed didn't start".into(),
        level,
        action: action.map(str::to_owned),
        // As the label's length sized "Retry" before it was measured.
        button_w: 64,
        leaving: false,
    }
}

fn tooltip() -> TooltipView {
    TooltipView {
        text: "Restart what runs here".into(),
        key: Some("Ctrl+B a".into()),
        anchor: Rect::new((300, 48).into(), (24, 24).into()),
        rows: Vec::new(),
        fields: Vec::new(),
    }
}

fn placeholder(timed_out: bool) -> PlaceholderView {
    PlaceholderView {
        rect: Rect::new((200, 48).into(), (480, 320).into()),
        label: "Zed".into(),
        since: Instant::now(),
        timed_out,
        closed: false,
        held: false,
    }
}

fn help() -> Help {
    Help {
        prefix: "Ctrl+B".into(),
        rows: vec![
            ("c".into(), "new tab".into()),
            ("x / Shift+X".into(), "close tab".into()),
            ("n / p".into(), "next / previous tab".into()),
            ("Shift+P".into(), "projects".into()),
            ("q".into(), "quit".into()),
        ],
        presses: {
            let act = |a| Some(HelpPress::Action(a));
            vec![
                vec![act(Action::NewTab)],
                vec![act(Action::CloseTab); 2],
                vec![act(Action::NextTab), act(Action::PrevTab)],
                vec![act(Action::Projects)],
                vec![act(Action::Quit)],
            ]
        },
    }
}

fn switcher() -> SwitcherView {
    let row = |icon,
               label: &str,
               detail: &str,
               kind: &'static str,
               section: Option<&'static str>,
               hits: &[usize]| SwitcherRow {
        icon,
        label: label.into(),
        detail: detail.into(),
        kind,
        section,
        label_hits: hits.to_vec(),
        detail_hits: Vec::new(),
    };
    SwitcherView {
        query: "ger".into(),
        hint: "Switch to…",
        categories: vec!["All", "Workspaces", "Tabs", "Agents", "Folders"],
        category: 0,
        category_hover: Some(2),
        hint_hover: None,
        part: 0.0,
        hints: vec![
            ("Enter".into(), "open".into()),
            ("Alt+Enter".into(), "in a new tab".into()),
            ("Tab".into(), "category".into()),
            ("Esc".into(), "close".into()),
        ],
        rows: vec![
            row(
                ItemIcon::Workspace(WorkspaceKind::Project),
                "gerdr",
                "~/Personal/gerdr · main",
                "workspace",
                Some("Workspaces"),
                &[0, 1, 2],
            ),
            row(ItemIcon::Tab, "2 zsh", "gerdr", "tab", Some("Tabs"), &[]),
            row(
                ItemIcon::Agent(
                    super::Badge {
                        icon: crate::agent_icons::AgentIcon::Claude,
                        dot: super::Dot::Working,
                    },
                    AgentStatus::Working,
                ),
                "claude",
                "gerdr",
                "agent",
                Some("Agents"),
                &[],
            ),
            row(
                ItemIcon::Folder,
                "~/Personal/herdr",
                "folder",
                "folder",
                Some("Recent folders"),
                &[],
            ),
            row(
                ItemIcon::App,
                "firefox",
                "firefox http://localhost",
                "app",
                None,
                &[],
            ),
        ],
        selected: Some(0),
        hover: Some(3),
        bar: None,
    }
}

fn welcome() -> WelcomeView {
    let row = |name: &str, running, current| ProjectRow {
        name: name.into(),
        detail: "3 repos · ~/Work/aft".into(),
        ago: "2h ago".into(),
        running,
        current,
    };
    WelcomeView {
        greeting: "Good evening, lgal".into(),
        subtitle: "Pick up where you left off".into(),
        query: "af".into(),
        rows: vec![
            row("aft", Some(true), true),
            row("a project with a very long name indeed", Some(false), false),
            row("gerdr", None, false),
        ],
        hits: vec![vec![0, 1], vec![], vec![]],
        selected: Some(1),
        hover: Some(WelcomeHit::Row(2)),
        hover_alpha: 16,
        naming: None,
        confirm_forget: None,
        note: Some("herdr isn't installed".into()),
        in_app: true,
        quit_keys: Some("Ctrl+B q".into()),
        scroll: None,
    }
}

fn quit() -> QuitView {
    let row = |label: &str, detail: &str, checked| QuitRow {
        label: label.into(),
        detail: detail.into(),
        checked,
    };
    QuitView {
        project: "aft".into(),
        rows: vec![
            row("Terminals and agents", "herdr session", true),
            row("Cursor", "kept running", false),
            row("Don't ask again", "", true),
        ],
        cursor: 1,
        selected: 1,
        hover: Some(QuitHit::Button(0)),
        switch_to: None,
    }
}

fn project_menu() -> ProjectMenuView {
    let row = |name: &str, running, current| ProjectRow {
        name: name.into(),
        detail: "3 repos · ~/Work/aft".into(),
        ago: "2h ago".into(),
        running,
        current,
    };
    let project = |row, digit| MenuEntry::Project {
        row,
        digit,
        renaming: None,
        confirm_forget: false,
        hits: vec![0, 1, 2],
    };
    ProjectMenuView {
        query: "blo".into(),
        entries: vec![
            MenuEntry::Section("Current".into()),
            project(row("aft", Some(true), true), None),
            MenuEntry::Section("Recent".into()),
            project(row("gerdr", Some(true), false), Some(1)),
            project(row("blog", Some(false), false), Some(2)),
            MenuEntry::New("New project \"blo\"…".into()),
            MenuEntry::Folder("~/Sites/blogging".into()),
        ],
        selected: Some(3),
        hover: Some(MenuHit::Entry(4)),
        form: None,
        settings: None,
        hints: vec![
            ("Enter".into(), "open".into()),
            ("Ctrl+N".into(), "new".into()),
            ("F2".into(), "rename".into()),
            ("Ctrl+E".into(), "commands".into()),
            ("Ctrl+O".into(), "project file".into()),
            ("Ctrl+Q".into(), "stop".into()),
            ("Del".into(), "forget".into()),
            ("Esc".into(), "close".into()),
        ],
        hover_alpha: 16,
        part: 0.0,
        bar: None,
    }
}

fn form() -> ProjectMenuView {
    ProjectMenuView {
        query: Line::default(),
        entries: Vec::new(),
        selected: None,
        hover: Some(MenuHit::Field(FormField::Browse)),
        form: Some(FormView {
            name: "blog-v2".into(),
            folder: "~/Sites/blog".into(),
            kind: WorkspaceKind::Project,
            focus: FormField::Folder,
            suggestions: vec!["~/Sites/blog".into(), "~/Sites/blog-old".into()],
            suggestion: Some(0),
            error: Some((FormField::Name, "A project named blog-v2 exists.".into())),
        }),
        settings: None,
        hints: vec![("Esc".into(), "back".into())],
        hover_alpha: 16,
        part: 0.0,
        bar: None,
    }
}

const SURFACES: [&str; 12] = [
    "sidebar",
    "tab_strip",
    "pane_bar",
    "help",
    "menu",
    "switcher",
    "welcome",
    "quit",
    "project_menu",
    "project_form",
    "toast",
    "agent_strip",
];

/// Every surface at (theme, scale), in `SURFACES` order.
fn inbox_item(
    id: &str,
    title: &str,
    subtitle: &str,
    icon: InboxIcon,
    tone: InboxTone,
    age: &str,
) -> InboxRow {
    InboxRow::Item(InboxLine {
        id: id.into(),
        title: title.into(),
        subtitle: subtitle.into(),
        icon,
        tone,
        age: age.into(),
        hidden: false,
        buttons: Vec::new(),
        marks: None,
    })
}

fn inbox() -> InboxView {
    let label = |icon, title: &str, count, hidden| InboxRow::Label {
        icon,
        title: title.into(),
        count,
        hidden,
        buttons: Vec::new(),
    };
    let button = |label: &str, key: &str| InboxButton {
        label: label.into(),
        key: key.into(),
    };
    use crate::agent_icons::AgentIcon;
    use InboxIcon as I;
    use InboxTone as T;
    let badge = |icon, dot| I::Badge(super::Badge { icon, dot });
    let mut rows = vec![
        label(I::Agent, "Agents", 3, 0),
        inbox_item(
            "a1",
            "claude",
            "gerdr · blocked · Allow edit?",
            badge(AgentIcon::Claude, super::Dot::Blocked),
            T::Blocked,
            "2m",
        ),
        inbox_item(
            "a2",
            "codex",
            "gerdr-inbox · done",
            badge(AgentIcon::Codex, super::Dot::Done),
            T::Done,
            "14m",
        ),
        inbox_item(
            "a3",
            "claude",
            "aft · working",
            badge(AgentIcon::Claude, super::Dot::Working),
            T::Working,
            "1h",
        ),
        {
            let mut row = label(I::Failed, "Services", 2, 0);
            if let InboxRow::Label { buttons, .. } = &mut row {
                *buttons = vec![button("Dismiss all", "Ctrl+Del")];
            }
            row
        },
        inbox_item(
            "s1",
            "TypeError: cannot read 'id' of undefined",
            "api · npm run dev · 3 times",
            I::Failed,
            T::Failed,
            "4m",
        ),
        inbox_item(
            "s2",
            "worker stopped",
            "api · cargo run --bin worker",
            I::Failed,
            T::Failed,
            "1h",
        ),
        // Folded to its count until shown (Alt+I).
        {
            let mut row = label(I::Failed, "Ignored errors", 3, 0);
            if let InboxRow::Label { buttons, .. } = &mut row {
                *buttons = vec![button("Show", "Alt+I")];
            }
            row
        },
        label(I::Repo, "Repos", 3, 1),
        {
            let mut row = inbox_item("g1", "gerdr", "gerdr · main", I::Repo, T::Attention, "");
            if let InboxRow::Item(line) = &mut row {
                line.marks = Some(super::InboxMarks {
                    dirty: true,
                    ahead: 2,
                    ..Default::default()
                });
            }
            row
        },
        inbox_item(
            "g2",
            "fix-scroll",
            "gerdr · worktree · merged",
            I::Worktree,
            T::Attention,
            "",
        ),
        inbox_item(
            "c2",
            "main",
            "gerdr · main · CI failed",
            I::Failed,
            T::Failed,
            "",
        ),
        label(I::PullRequest, "Pull requests", 2, 0),
        inbox_item(
            "c1",
            "Speed up popup paint",
            "gerdr · #41 · review asked",
            I::PullRequest,
            T::Review,
            "",
        ),
        inbox_item(
            "c3",
            "Group the inbox",
            "gerdr · #44 · CI failed",
            I::PullRequest,
            T::Failed,
            "",
        ),
        label(I::Ticket, "Tickets", 3, 0),
        inbox_item(
            "k1",
            "Inbox grouping",
            "o/tickets#12 · In progress",
            I::Ticket,
            T::Info,
            "",
        ),
        inbox_item(
            "k2",
            "Linear token scope",
            "ENG-41 · Todo",
            I::Ticket,
            T::Info,
            "",
        ),
        inbox_item(
            "t1",
            "Check the board read",
            "gerdr · todo",
            I::Todo,
            T::Info,
            "3d",
        ),
    ];
    if let InboxRow::Item(line) = &mut rows[1] {
        line.buttons = vec![button("Snooze", "Ctrl+H"), button("Dismiss", "Del")];
    }
    InboxView {
        project: Some("gerdr".into()),
        all: Some(true),
        placeholder: "Filter the inbox…".into(),
        total: rows.len(),
        rows,
        selected: Some(1),
        detail: Some(InboxDetailView {
            title: "claude".into(),
            subtitle: "gerdr · blocked · Allow edit?".into(),
            icon: InboxIcon::Badge(super::Badge {
                icon: crate::agent_icons::AgentIcon::Claude,
                dot: super::Dot::Blocked,
            }),
            tone: InboxTone::Blocked,
            fields: vec![
                ("Status".into(), "blocked".into()),
                ("Workspace".into(), "gerdr".into()),
                ("Model".into(), "opus".into()),
            ],
            block_title: Some("Last output".into()),
            block: vec![
                "Edit src/chrome/inbox.rs".into(),
                "Do you want to make this edit?".into(),
                "❯ 1. Yes".into(),
                "  2. No".into(),
            ],
            block_tail: true,
            buttons: vec![
                button("Go to", "Enter"),
                button("Snooze", "Ctrl+H"),
                button("Tomorrow", "Ctrl+Shift+H"),
                button("Dismiss", "Del"),
            ],
        }),
        hints: vec![
            ("Enter".into(), "go to".into()),
            ("Ctrl+N".into(), "todo".into()),
            ("Esc".into(), "close".into()),
        ],
        empty: "Nothing needs you".into(),
        ..InboxView::default()
    }
}

fn paint(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    let help = help();
    let help_layout = painter.help_layout(&help, 800, false, scale);
    let notice = toast(None, Level::Info);
    let strip = (900, super::tab_strip_height(super::Density::Comfortable)).into();
    vec![
        hash(painter.sidebar_view(
            &SidebarView {
                workspaces: workspaces(),
                agents: agents(),
                active: Some(0),
                project: Some("aft".into()),
                header_lit: true,
                features: Vec::new(),
                // The repositories button with its badge and ↑/↓ marks.
                state: SidebarState {
                    repos_changed: 2,
                    repos_ahead: true,
                    repos_behind: true,
                    ..SidebarState::default()
                },
            },
            (200, 400).into(),
            scale,
            p,
        )),
        hash(painter.tab_strip_view(&strip_view(), theme, strip, scale)),
        hash(painter.pane_bar(&pane_bar(), scale, p)),
        hash(painter.help_panel(&help, &help_layout, None, scale, p)),
        hash(painter.menu(&menu(), scale, p)),
        hash(painter.switcher(&switcher(), 640, scale, p)),
        hash(painter.welcome(&welcome(), (800, 600).into(), scale, p)),
        hash(painter.quit_dialog(&quit(), (800, 600).into(), scale, p)),
        hash(painter.project_menu(&project_menu(), (800, 600).into(), scale, p)),
        hash(painter.project_menu(&form(), (800, 600).into(), scale, p)),
        hash(painter.toast(&notice, 360, false, scale, p)),
        hash(painter.agent_strip_part(&super::sidebar::StripKey {
            picks: vec![
                crate::agent_icons::AgentIcon::Claude,
                crate::agent_icons::AgentIcon::T3,
                crate::agent_icons::AgentIcon::Cursor,
            ],
            pick: Some(1),
            size: (66, 24).into(),
            theme,
            scale,
        })),
    ]
}

/// The surfaces the motion work added, apart so the table above keeps
/// its shape: a menu with hints, a pinned help panel, a toast with a
/// button, a tooltip, the placeholder waiting and timed out, the spinner.
const OVERLAYS: [&str; 11] = [
    "full_menu",
    "help_pinned",
    "toast_action",
    "tooltip",
    "placeholder",
    "placeholder_timed_out",
    "spinner",
    "blink_dot",
    "pane_bar_web",
    "pane_bar_waiting",
    "pane_bar_unfocused",
];

fn paint_overlays(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    let help = help();
    let pinned = painter.help_layout(&help, 800, true, scale);
    let tip = tooltip();
    let tip_w = painter.tooltip_width(&tip, 400, scale);
    vec![
        hash(painter.menu(&full_menu(), scale, p)),
        hash(painter.help_panel(&help, &pinned, None, scale, p)),
        hash(painter.toast(&toast(Some("Retry"), Level::Error), 360, true, scale, p)),
        hash(painter.tooltip(&tip, tip_w, scale, p)),
        hash(painter.placeholder(&placeholder(false), None, scale, p)),
        hash(painter.placeholder(&placeholder(true), Some(PlaceholderButton::Retry), scale, p)),
        hash(painter.spinner(5, scale, p)),
        hash(super::blink_dot(super::Dot::Working, scale, p)),
        hash(painter.pane_bar(&pane_bar_web(), scale, p)),
        hash(painter.pane_bar(&pane_bar_waiting(), scale, p)),
        hash(painter.pane_bar(&pane_bar_unfocused(), scale, p)),
    ]
}

/// A web page's bar, the copy button hovered.
fn pane_bar_web() -> PaneBarView {
    let pane = Rect::new((0, 0).into(), (900, 400).into());
    let buttons = BarButtons::Web { history: true };
    PaneBarView {
        rect: super::pane_bar_rects(pane).0,
        label: "http://localhost:3773/7a88cf7d-c216-4867/d0734377-8eac".into(),
        note: None,
        status: BarStatus::Idle,
        can_restart: true,
        can_back: true,
        buttons,
        zoom: Some(false),
        hover: Some(BarButton::CopyUrl),
        ..pane_bar()
    }
}

/// A stage agent's terminal, waiting for its prompt, with its Chat button.
fn pane_bar_waiting() -> PaneBarView {
    let pane = Rect::new((0, 0).into(), (900, 400).into());
    let buttons = BarButtons::Agent {
        chat: false,
        waiting: true,
    };
    PaneBarView {
        rect: super::pane_bar_rects(pane).0,
        label: "claude --add-dir /d/my-feature".into(),
        note: Some("waiting for its prompt"),
        status: BarStatus::Running,
        can_restart: false,
        can_stop: true,
        buttons,
        zoom: Some(false),
        hover: None,
        ..pane_bar()
    }
}

/// A terminal's bar whose pane hasn't the focus, something running in it.
fn pane_bar_unfocused() -> PaneBarView {
    PaneBarView {
        note: None,
        status: BarStatus::Working,
        can_stop: true,
        zoom: Some(false),
        focused: false,
        hover: None,
        ..pane_bar()
    }
}

const CASES: [(Theme, f64); 4] = [
    (Theme::Dark, 1.0),
    (Theme::Dark, 2.0),
    (Theme::Light, 1.0),
    (Theme::Light, 2.0),
];

/// The recorded hashes, per case in `CASES`, per surface in `SURFACES`.
const GOLDEN: [[u64; 12]; 4] = [
    // Dark at 1
    [
        0xdb314f0f15e4559d, // sidebar
        0xa22a985552596599, // tab_strip
        0x8d8e5620f1434b02, // pane_bar
        0xe42d0016f78b5be5, // help
        0xd5efbb52273d2550, // menu
        0xdcd127368f8b7e3d, // switcher
        0xc81959b4b56723a9, // welcome
        0x4ffaff1032915ab3, // quit
        0xc14abffc64c90067, // project_menu
        0x96dd24d87bbd5056, // project_form
        0xc62d73c988a96a2c, // toast
        0x1b80b690e340528a, // agent_strip
    ],
    // Dark at 2
    [
        0x2f9bdf462c5b3cf6, // sidebar
        0x1e0809759c35420a, // tab_strip
        0x90ee51f8d14bf663, // pane_bar
        0x15fe992f5155a75d, // help
        0xebd62797ace63234, // menu
        0xa990b4e36df3a494, // switcher
        0x74c455e566770a4c, // welcome
        0x2f1a360d791dd680, // quit
        0xef19fe8f804ec9d5, // project_menu
        0x1b0b6f4e613701ff, // project_form
        0xa13987ebc78a12de, // toast
        0x95f9334ab0c42dc0, // agent_strip
    ],
    // Light at 1
    [
        0xaaabb650e557f8c8, // sidebar
        0x533365a2916c9b4b, // tab_strip
        0x3c25b5f106eb80db, // pane_bar
        0x85e7a8c233b51426, // help
        0xfabf24c25ead538e, // menu
        0x5a937f6eb8163425, // switcher
        0x22c637a65f497b4a, // welcome
        0x6d4ef567787e76ed, // quit
        0x89525be40c8a000a, // project_menu
        0x499cf564982d0b29, // project_form
        0x6caa7ae56f93dad4, // toast
        0xa37db337f4fcec5f, // agent_strip
    ],
    // Light at 2
    [
        0x04be95e887db6af8, // sidebar
        0xf654bc4135568adc, // tab_strip
        0xeefbb060d193a419, // pane_bar
        0x9e0ac87f5c27c71c, // help
        0xadc66d3812572c4a, // menu
        0x8d2715b3225dea07, // switcher
        0x1c70a6361710c852, // welcome
        0xe1f4b8cf06fe4e37, // quit
        0xd44507e0a330ce75, // project_menu
        0x85440a4af6381169, // project_form
        0x91103b3094554747, // toast
        0x8aeabdb50084b1d6, // agent_strip
    ],
];

/// The recorded hashes of `OVERLAYS`, per case in `CASES`.
const GOLDEN_OVERLAYS: [[u64; 11]; 4] = [
    // Dark at 1
    [
        0x89ed7bf59ee42065, // full_menu
        0x1f7be60c5749ad73, // help_pinned
        0x8f4ea6aaca2ed05f, // toast_action
        0xfd3ac3587fa37c78, // tooltip
        0xd637120c2c484793, // placeholder
        0x908d1455cfcf17c1, // placeholder_timed_out
        0x7a426c6be23d66a6, // spinner
        0x5f7da883de161583, // blink_dot
        0x13717ff712de3f5a, // pane_bar_web
        0x8b3bffa06d244dfc, // pane_bar_waiting
        0x8507cf82a0901980, // pane_bar_unfocused
    ],
    // Dark at 2
    [
        0xcb53ae022a6a3d85, // full_menu
        0x7afa5cf665e54c83, // help_pinned
        0xbea4908e5289cf9f, // toast_action
        0xb725bdf0b62b45ac, // tooltip
        0xc39ed1ba9d6067bf, // placeholder
        0x4bd8f8fb807a3cd3, // placeholder_timed_out
        0x2f28c94a7002e520, // spinner
        0x3938d7cb530659b5, // blink_dot
        0x15ee5443b0556d0d, // pane_bar_web
        0xfd45cffc41fd0a61, // pane_bar_waiting
        0x0f7db27b41457685, // pane_bar_unfocused
    ],
    // Light at 1
    [
        0xc7be6f3e0c7b21ef, // full_menu
        0x919e6dd748618e1d, // help_pinned
        0x36e05eda9f5ffa87, // toast_action
        0x0e0cd3cdd699260a, // tooltip
        0x1ac440e0e97ea1c5, // placeholder
        0x3d3c67e6398a2815, // placeholder_timed_out
        0x6cb91debc8d3c473, // spinner
        0x55c89cc3e0120adb, // blink_dot
        0xcb26e5962f9547b8, // pane_bar_web
        0x6acc7b0435e2c29e, // pane_bar_waiting
        0x5fe277666338fff7, // pane_bar_unfocused
    ],
    // Light at 2
    [
        0xcada18e981680119, // full_menu
        0xe0afa42528d95d40, // help_pinned
        0xb7a9cadeac643d5c, // toast_action
        0x7e5ad0b91e5005e1, // tooltip
        0x4ca48ee1be70c283, // placeholder
        0x763d3de0476280d9, // placeholder_timed_out
        0x5ad35f5c0797002e, // spinner
        0x349f0f6edf45454d, // blink_dot
        0x1b94dd2d4ab88554, // pane_bar_web
        0x84eb9db68257cce6, // pane_bar_waiting
        0x2e4a5a9061bd0762, // pane_bar_unfocused
    ],
];

/// How many threads the tests may keep busy: `RUST_TEST_THREADS`, which
/// `.cargo/config.toml` sets, else every core.
fn test_threads() -> usize {
    std::env::var("RUST_TEST_THREADS")
        .ok()
        .and_then(|n| n.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
}

/// Paint threads at work now, in all the tests running together.
static PAINTING: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// A paint thread's turn: it waits until fewer than `limit` are at work,
/// and gives the turn back when dropped, also on a panic.
struct PaintTurn;

impl PaintTurn {
    fn take(limit: usize) -> Self {
        let (painting, freed) = &PAINTING;
        let mut painting = painting.lock().unwrap();
        while *painting >= limit {
            painting = freed.wait(painting).unwrap();
        }
        *painting += 1;
        Self
    }
}

impl Drop for PaintTurn {
    fn drop(&mut self) {
        let (painting, freed) = &PAINTING;
        *painting.lock().unwrap() -= 1;
        freed.notify_one();
    }
}

/// Run `job` for every index below `count`, on `test_threads` threads, and
/// give the results in index order. The limit holds across tests: without
/// it, tests running together would each take that many, and fill every
/// core. A `Painter` is not `Sync`, so each thread makes its own; the slow
/// surfaces take most of the test run in a dev build.
pub(super) fn fan_out<T: Send>(count: usize, job: impl Fn(&Painter, usize) -> T + Sync) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let limit = test_threads();
    let mut done: Vec<(usize, T)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..limit.min(count))
            .map(|_| {
                scope.spawn(|| {
                    let _turn = PaintTurn::take(limit);
                    let painter = Painter::new();
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= count {
                            return done;
                        }
                        done.push((i, job(&painter, i)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    done.sort_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, result)| result).collect()
}

/// Check `paint` of every case against `golden`, a thread per case.
fn check(
    surfaces: &[&str],
    golden: &[&[u64]],
    paint: impl Fn(&Painter, Theme, f64) -> Vec<u64> + Sync,
) {
    let got = fan_out(CASES.len(), |painter, case| {
        assert!(painter.has_font(), "the golden hashes need a font");
        let (theme, scale) = CASES[case];
        paint(painter, theme, scale)
    });
    compare(surfaces, golden, &got);
}

/// Check `paint` of every surface of every case against `golden`, for
/// surfaces slow enough that a thread per case is not enough.
fn check_each(
    surfaces: &[&str],
    golden: &[&[u64]],
    paint: impl Fn(&Painter, Theme, f64, usize) -> u64 + Sync,
) {
    let hashes = fan_out(CASES.len() * surfaces.len(), |painter, i| {
        assert!(painter.has_font(), "the golden hashes need a font");
        let (theme, scale) = CASES[i / surfaces.len()];
        paint(painter, theme, scale, i % surfaces.len())
    });
    let got: Vec<Vec<u64>> = hashes.chunks(surfaces.len()).map(<[u64]>::to_vec).collect();
    compare(surfaces, golden, &got);
}

/// Compare `got` with `golden`, per case in `CASES`, printing the table to
/// record when any surface paints differently.
fn compare(surfaces: &[&str], golden: &[&[u64]], got: &[Vec<u64>]) {
    let mut table = String::new();
    let mut same = true;
    for (((theme, scale), want), got) in CASES.into_iter().zip(golden).zip(got) {
        table += &format!("    // {theme:?} at {scale}\n    [\n");
        for (i, (surface, hash)) in surfaces.iter().zip(got).enumerate() {
            if *hash != want[i] {
                same = false;
                eprintln!(
                    "{surface} ({theme:?}, {scale}): {hash:#018x}, recorded {:#018x}",
                    want[i]
                );
            }
            table += &format!("        {hash:#018x}, // {surface}\n");
        }
        table += "    ],\n";
    }
    assert!(
        same,
        "the chrome paints differently; if on purpose, record:\n{table}"
    );
}

#[test]
fn surfaces_paint_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN.iter().map(|row| row.as_slice()).collect();
    check(&SURFACES, &golden, paint);
}

#[test]
fn overlays_paint_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_OVERLAYS.iter().map(|row| row.as_slice()).collect();
    check(&OVERLAYS, &golden, paint_overlays);
}

/// Write `buffer` (`w` pixels wide) as `<dir>/<name>.ppm`, laid over a
/// grey ground where it is clear, to look at it.
fn dump(dir: &std::path::Path, name: &str, mut buffer: MemoryRenderBuffer, w: usize) {
    let mut out = Vec::new();
    let _ = buffer.render().draw(|mem| {
        let h = mem.len() / 4 / w;
        out.extend_from_slice(format!("P6\n{w} {h}\n255\n").as_bytes());
        for px in mem.as_chunks::<4>().0 {
            let a = px[3] as u32;
            // Premultiplied over a grey ground.
            for c in [px[2], px[1], px[0]] {
                out.push((c as u32 + 0x80 * (255 - a) / 255) as u8);
            }
        }
        Ok::<_, ()>(Vec::new())
    });
    let _ = std::fs::write(dir.join(format!("{name}.ppm")), out);
}

/// With `GERDR_DUMP_DIR` set, write the motion surfaces there as images
/// to look at (both themes, scale 1 and 2). Otherwise nothing.
#[test]
fn dump_overlays() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    for (theme, scale) in CASES {
        let p = theme.palette();
        let px = |v: i32| (v as f64 * scale).round() as usize;
        let tag = format!("{}_{}", theme.name(), scale);
        let help = help();
        let pinned = painter.help_layout(&help, 800, true, scale);
        let tip = tooltip();
        let tip_w = painter.tooltip_width(&tip, 400, scale);
        let menu = full_menu();
        let (panel, _) = super::menu_layout(menu.anchor, &menu.rows, (2000, 2000).into());
        let shadow = 2 * super::SHADOW;
        let waiting = placeholder(false);
        dump(
            &dir,
            &format!("menu_{tag}"),
            painter.menu(&menu, scale, p),
            px(panel.size.w + shadow),
        );
        dump(
            &dir,
            &format!("help_pinned_{tag}"),
            painter.help_panel(&help, &pinned, None, scale, p),
            px(pinned.size.w + shadow),
        );
        for (name, hover) in [("row", HelpHit::Row(0)), ("key", HelpHit::Key(2, 1))] {
            dump(
                &dir,
                &format!("help_hover_{name}_{tag}"),
                painter.help_panel(&help, &pinned, Some(hover), scale, p),
                px(pinned.size.w + shadow),
            );
        }
        dump(
            &dir,
            &format!("toast_{tag}"),
            painter.toast(&toast(Some("Retry"), Level::Error), 360, true, scale, p),
            px(360 + shadow),
        );
        dump(
            &dir,
            &format!("toast_info_{tag}"),
            painter.toast(&toast(None, Level::Info), 360, false, scale, p),
            px(360 + shadow),
        );
        dump(
            &dir,
            &format!("switcher_{tag}"),
            painter.switcher(&switcher(), 640, scale, p),
            px(640 + shadow),
        );
        for (name, bar) in [
            ("pane_bar", pane_bar()),
            ("pane_bar_web", pane_bar_web()),
            ("pane_bar_waiting", pane_bar_waiting()),
            ("pane_bar_unfocused", pane_bar_unfocused()),
        ] {
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.pane_bar(&bar, scale, p),
                px(bar.rect.size.w),
            );
        }
        dump(
            &dir,
            &format!("switcher_hint_{tag}"),
            painter.switcher(
                &SwitcherView {
                    hint_hover: Some(1),
                    ..switcher()
                },
                640,
                scale,
                p,
            ),
            px(640 + shadow),
        );
        let confirm = super::ConfirmView {
            title: "Close \u{201c}claude\u{201d}?".into(),
            body: "It is working: closing its pane stops it".into(),
            go: "Close pane",
            selected: 1,
            hover: None,
            loses: true,
        };
        let confirm_w = super::confirm_layout((800, 600).into()).panel.size.w;
        dump(
            &dir,
            &format!("confirm_{tag}"),
            painter.confirm_dialog(&confirm, (800, 600).into(), scale, p),
            px(confirm_w + shadow),
        );
        let finish = super::ConfirmView {
            title: "Finish \u{201c}Bulk export\u{201d} without shipping?".into(),
            body: "Nothing gets pushed or merged\nfeature/bulk-export and its worktree stay\n\
                   api has uncommitted changes"
                .into(),
            go: "Finish",
            selected: 1,
            hover: None,
            loses: false,
        };
        dump(
            &dir,
            &format!("confirm_finish_{tag}"),
            painter.confirm_dialog(&finish, (800, 600).into(), scale, p),
            px(confirm_w + shadow),
        );
        let old_agents = super::ConfirmView {
            title: "Close 6 old agent tabs?".into(),
            body: "Their agents are done; closing the tabs ends their sessions:\n\
                   claude \u{b7} Discover\nclaude \u{b7} Plan\nclaude \u{b7} Plan review\n\
                   codex \u{b7} Build\nand 2 more"
                .into(),
            go: "Close tabs",
            selected: 1,
            hover: None,
            loses: true,
        };
        dump(
            &dir,
            &format!("confirm_old_agents_{tag}"),
            painter.confirm_dialog(&old_agents, (800, 600).into(), scale, p),
            px(confirm_w + shadow),
        );
        let row = |path: &str, files, link, clash| super::CheckoutRow {
            path: path.into(),
            files,
            link,
            clash,
        };
        for (name, rows, can_bring) in [
            (
                "checkout",
                vec![
                    row("dev/measure_optimize_detail.py", 1, false, false),
                    row("dev/measure_optimize_tree.py", 1, false, false),
                    row("app/services/new/", 3, false, false),
                    row("venv", 1, true, false),
                ],
                true,
            ),
            (
                "checkout_clash",
                vec![
                    row("dev/measure.py", 1, false, false),
                    row("venv", 1, true, true),
                ],
                true,
            ),
        ] {
            let view = super::CheckoutView {
                branch: "feature/optimization-performance".into(),
                main: "services-api".into(),
                worktree: "services-api-perf".into(),
                rows,
                can_bring,
                selected: if can_bring { 2 } else { 1 },
                hover: None,
            };
            let w = super::checkout_layout((800, 600).into(), view.rows.len())
                .panel
                .size
                .w;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.checkout_dialog(&view, (800, 600).into(), scale, p),
                px(w + shadow),
            );
        }
        let quit_w = super::quit_layout((800, 600).into(), quit().rows.len(), false)
            .panel
            .size
            .w;
        dump(
            &dir,
            &format!("quit_{tag}"),
            painter.quit_dialog(&quit(), (800, 600).into(), scale, p),
            px(quit_w + shadow),
        );
        dump(
            &dir,
            &format!("tooltip_{tag}"),
            painter.tooltip(&tip, tip_w, scale, p),
            px(tip_w + shadow),
        );
        dump(
            &dir,
            &format!("placeholder_{tag}"),
            painter.placeholder(&waiting, None, scale, p),
            px(waiting.rect.size.w),
        );
        dump(
            &dir,
            &format!("placeholder_late_{tag}"),
            painter.placeholder(&placeholder(true), Some(PlaceholderButton::Retry), scale, p),
            px(waiting.rect.size.w),
        );
        let inbox_size = (1200, 760).into();
        let inbox_panel = super::inbox_layout(&inbox(), inbox_size).panel;
        dump(
            &dir,
            &format!("inbox_{tag}"),
            painter.inbox(&inbox(), inbox_size, scale, p),
            px(inbox_panel.size.w + shadow),
        );
        // Its first workspace with a blocked agent and errors of its
        // services: both pills on its header.
        let mut erring = workspaces();
        erring[0].errors = 3;
        dump(
            &dir,
            &format!("sidebar_agents_{tag}"),
            painter.sidebar_view(
                &SidebarView {
                    workspaces: erring,
                    agents: many_agents(),
                    active: Some(0),
                    project: Some("aft".into()),
                    header_lit: false,
                    features: Vec::new(),
                    state: SidebarState::default(),
                },
                (260, 520).into(),
                scale,
                p,
            ),
            px(260),
        );
        // A header under the pointer, on its git marks: lit, as they open
        // the repositories panel.
        dump(
            &dir,
            &format!("sidebar_git_marks_{tag}"),
            painter.sidebar_view(
                &SidebarView {
                    workspaces: workspaces(),
                    agents: many_agents(),
                    active: Some(0),
                    project: Some("aft".into()),
                    header_lit: false,
                    features: Vec::new(),
                    state: SidebarState {
                        rows: vec![RowState {
                            hover: 16,
                            git: true,
                            ..RowState::default()
                        }],
                        ..SidebarState::default()
                    },
                },
                (260, 520).into(),
                scale,
                p,
            ),
            px(260),
        );
        // A header under the pointer, on its agent button.
        let hovered = RowState {
            hover: 16,
            agent: true,
            ..RowState::default()
        };
        dump(
            &dir,
            &format!("sidebar_hover_{tag}"),
            painter.sidebar_view(
                &SidebarView {
                    workspaces: workspaces(),
                    agents: many_agents(),
                    active: Some(0),
                    project: Some("aft".into()),
                    header_lit: false,
                    features: Vec::new(),
                    state: SidebarState {
                        rows: vec![hovered],
                        ..SidebarState::default()
                    },
                },
                (260, 520).into(),
                scale,
                p,
            ),
            px(260),
        );
        // The same, its AI button's strip of agents open, one lit.
        let mut picking = workspaces();
        picking[0].picks = vec![
            crate::agent_icons::AgentIcon::Claude,
            crate::agent_icons::AgentIcon::T3,
            crate::agent_icons::AgentIcon::Cursor,
            crate::agent_icons::AgentIcon::OpenCode,
        ];
        // An errors pill and a worktree count it must leave in sight.
        picking[0].errors = 1;
        if let Some(git) = picking[0].git.as_mut() {
            git.worktrees = vec![crate::git::Worktree::default(); 2];
        }
        dump(
            &dir,
            &format!("sidebar_agent_strip_{tag}"),
            painter.sidebar_view(
                &SidebarView {
                    workspaces: picking,
                    agents: many_agents(),
                    active: Some(0),
                    project: Some("aft".into()),
                    header_lit: false,
                    features: Vec::new(),
                    state: SidebarState {
                        rows: vec![RowState {
                            pick: Some(1),
                            agent: false,
                            ..hovered
                        }],
                        ..SidebarState::default()
                    },
                },
                (260, 520).into(),
                scale,
                p,
            ),
            px(260),
        );
        let strip_h = super::tab_strip_height(super::Density::Comfortable);
        dump(
            &dir,
            &format!("tab_strip_{tag}"),
            painter.tab_strip_view(&strip_view(), theme, (900, strip_h).into(), scale),
            px(900),
        );
        dump(
            &dir,
            &format!("spinner_{tag}"),
            painter.spinner(5, scale, p),
            px(super::SPINNER_SIZE),
        );
    }
}

/// A footer and its tasks panel with a task of each kind.
fn status_tasks() -> Vec<super::TaskView> {
    let task = |title: &str, chip: &str, detail: &str, dot, buttons: &[&str]| super::TaskView {
        title: title.into(),
        chip: chip.into(),
        detail: detail.into(),
        dot,
        buttons: buttons.iter().map(|b| (*b).to_owned()).collect(),
    };
    vec![
        task(
            "T3 Code server",
            "T3 Code",
            "0.1.6 · http://127.0.0.1:3773 · up 2h 5m",
            super::Dot::Done,
            &["Open", "Restart", "Stop", "Log"],
        ),
        task(
            "Git: fetch in gerdr",
            "gerdr: fetch",
            "/home/me/gerdr/.git",
            super::Dot::Working,
            &["Repositories"],
        ),
        task(
            "T3 Code 0.1.7 waits to start",
            "T3 Code 0.1.7 waits",
            "Restarts once 2 threads are done",
            super::Dot::Blocked,
            &["Restart now"],
        ),
    ]
}

fn status_footer() -> super::FooterView {
    super::FooterView {
        tasks: status_tasks(),
        hover: Some(super::FooterHit::Task(0)),
    }
}

fn status_panel() -> super::TasksView {
    super::TasksView {
        title: "Background tasks",
        rows: status_tasks(),
        first: 0,
        selected: Some(0),
        button: 0,
        hover: Some(super::TasksHit::Button(0, 1)),
        hints: [
            ("Up / Down", "move"),
            ("Tab", "next button"),
            ("Enter", "open"),
            ("Esc", "close"),
        ]
        .map(|(k, a)| (k.to_owned(), a.to_owned()))
        .into(),
    }
}

/// The Phone connections dialog: the server under its unit, a phone
/// connected, one seen an hour ago with notifications, one blocked.
fn phones_panel() -> super::TasksView {
    use crate::phone_devices::{Device, Seen, Server};
    let now = 1_000_000;
    let device = |ip: &str, name: &str, ago: u64, blocked| Device {
        ip: ip.into(),
        name: name.into(),
        login: "me@example.com".into(),
        first: 0,
        last: now - ago,
        blocked,
    };
    let seen = Seen {
        server: Some(Server {
            pid: 1,
            port: 8390,
            since: now - 7500,
        }),
        in_unit: true,
        unit: true,
        devices: vec![
            device("100.88.1.2", "pixel-8", 3600, false),
            device("100.88.1.3", "iphone-15", 2, false),
            device("100.88.1.4", "old-tablet", 86_400 * 3, true),
        ],
        error: None,
        notified: vec!["100.88.1.2".into()],
    };
    super::TasksView {
        title: "Phone connections",
        rows: seen
            .rows(now)
            .iter()
            .map(crate::tasks::Task::view)
            .collect(),
        first: 0,
        selected: Some(2),
        button: 0,
        hover: Some(super::TasksHit::Button(1, 0)),
        hints: [
            ("Up / Down", "move"),
            ("Tab", "next button"),
            ("Enter", "block"),
            ("Esc", "close"),
        ]
        .map(|(k, a)| (k.to_owned(), a.to_owned()))
        .into(),
    }
}

/// A feature workspace's tab strip: its panel tab first and shown, with
/// no "×", then the usual tabs.
fn pinned_strip_view() -> TabStripView {
    let mut tabs = tabs();
    tabs.insert(
        0,
        TabView {
            title: "Feature".into(),
            pinned: true,
            stage: Some("Code review"),
            ..Default::default()
        },
    );
    TabStripView::plain(tabs, 0, false, ThemeMode::System)
}

fn paint_pinned(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let strip = (900, super::tab_strip_height(super::Density::Comfortable)).into();
    vec![hash(painter.tab_strip_view(
        &pinned_strip_view(),
        theme,
        strip,
        scale,
    ))]
}

const PINNED: [&str; 1] = ["pinned_tab_strip"];

/// The recorded hashes of `PINNED`, per case in `CASES`.
const GOLDEN_PINNED: [[u64; 1]; 4] = [
    // Dark at 1
    [
        0x69dc9d6154305a1b, // pinned_tab_strip
    ],
    // Dark at 2
    [
        0xf482454f83ae5fef, // pinned_tab_strip
    ],
    // Light at 1
    [
        0x628549233c4dcc9d, // pinned_tab_strip
    ],
    // Light at 2
    [
        0x776a8fad1bfde131, // pinned_tab_strip
    ],
];

#[test]
fn a_pinned_tab_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_PINNED.iter().map(|row| row.as_slice()).collect();
    check(&PINNED, &golden, paint_pinned);
}

/// With `GERDR_DUMP_DIR` set, write the tab strip with a pinned tab there.
#[test]
fn dump_pinned() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    for (theme, scale) in CASES {
        let strip = (900, super::tab_strip_height(super::Density::Comfortable)).into();
        dump(
            &dir,
            &format!("pinned_tab_strip_{}_{}", theme.name(), scale),
            painter.tab_strip_view(&pinned_strip_view(), theme, strip, scale),
            (900.0 * scale).round() as usize,
        );
    }
}

const STATUS_OUTPUT: (i32, i32) = (1200, 700);

fn paint_status(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    let footer = super::status_bar_height(super::Density::Comfortable);
    vec![
        hash(painter.footer(&status_footer(), (STATUS_OUTPUT.0, footer).into(), scale, p)),
        hash(painter.tasks_panel(&status_panel(), STATUS_OUTPUT.into(), footer, scale, p)),
        hash(painter.tasks_panel(&phones_panel(), STATUS_OUTPUT.into(), footer, scale, p)),
    ]
}

const STATUS: [&str; 3] = ["footer", "tasks_panel", "phones_panel"];

/// The recorded hashes of `STATUS`, per case in `CASES`.
const GOLDEN_STATUS: [[u64; 3]; 4] = [
    // Dark at 1
    [
        0x9986ceaa3e0e0f45, // footer
        0x54f417b10d58492d, // tasks_panel
        0x5cee2cc62bc6f3f2, // phones_panel
    ],
    // Dark at 2
    [
        0xe0c82af6985c4765, // footer
        0xde664246ac277752, // tasks_panel
        0x807341bd1ff3754a, // phones_panel
    ],
    // Light at 1
    [
        0x452f8072b8702445, // footer
        0xe146bb86f7f3b882, // tasks_panel
        0x58e8a72699165964, // phones_panel
    ],
    // Light at 2
    [
        0x0ac1a2638ae1359b, // footer
        0x563c054180dc8ccd, // tasks_panel
        0x37c7c998a78ade68, // phones_panel
    ],
];

#[test]
fn the_footer_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_STATUS.iter().map(|row| row.as_slice()).collect();
    check(&STATUS, &golden, paint_status);
}

/// With `GERDR_DUMP_DIR` set, write the footer and the tasks panel there.
#[test]
fn dump_status() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    let footer = super::status_bar_height(super::Density::Comfortable);
    for (theme, scale) in CASES {
        let p = theme.palette();
        let px = |v: i32| (v as f64 * scale).round() as usize;
        let tag = format!("{}_{}", theme.name(), scale);
        dump(
            &dir,
            &format!("footer_{tag}"),
            painter.footer(&status_footer(), (STATUS_OUTPUT.0, footer).into(), scale, p),
            px(STATUS_OUTPUT.0),
        );
        for (name, view) in [
            ("tasks_panel", status_panel()),
            ("phones_panel", phones_panel()),
        ] {
            let panel = super::tasks_layout(&view, STATUS_OUTPUT.into(), footer).panel;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.tasks_panel(&view, STATUS_OUTPUT.into(), footer, scale, p),
                px(panel.size.w + 2 * super::SHADOW),
            );
        }
    }
}

/// The new feature dialog over three repositories, the second unticked,
/// and a linked project's one, the keyboard on the title.
fn new_feature() -> super::NewFeatureView {
    let repo = |name: &str, base: &str, touched: bool| super::NewFeatureRepo {
        name: name.into(),
        place: if touched { "linked" } else { "left out" }.into(),
        touched,
        fixed: false,
        base: Some(format!("from {base}")),
    };
    super::NewFeatureView {
        heading: "New feature in \u{201c}aft\u{201d}".into(),
        title: Line::from("Bulk export of invoices"),
        hint: super::TITLE_HINT,
        field_focus: true,
        focus: None,
        slots: 4,
        root: "~/Work/aft".into(),
        root_failed: false,
        projects: "Projects: aft (home) \u{b7} billing".into(),
        link: super::LINK_BUTTON[0],
        repos: vec![
            repo("api", "dev", true),
            repo("shared", "main", false),
            repo("web", "feature/a-branch-name-too-long-for-its-button", true),
            repo("billing: ledger", "main", false),
        ],
        top: 0,
        error: None,
        hover: Some(super::NewFeatureHit::Button(1)),
        keys: vec![
            ("Tab", "next"),
            ("Space", "tick"),
            ("Alt+B", "base branch"),
            ("Ctrl+Shift+L", "link project"),
            ("Esc", "clear"),
        ],
        create: super::NEW_FEATURE_BUTTONS[super::CREATE],
        choosing: false,
        reviewers: Vec::new(),
        reviewer_focus: false,
    }
}

/// The new feature dialog's list of a repository's branches, filtered, the
/// keyboard on the one it starts from.
fn new_feature_bases() -> super::NewFeatureView {
    let row = |name: &str, place: &str, touched| super::NewFeatureRepo {
        name: name.into(),
        place: place.into(),
        touched,
        fixed: false,
        base: None,
    };
    super::NewFeatureView {
        title: Line::from("de"),
        hint: "Filter branches",
        focus: Some(0),
        slots: 4,
        heading: "Base branch of \u{201c}assettag-mobile\u{201d}".into(),
        root: "Pick the branch assettag-mobile starts from".into(),
        choosing: true,
        repos: vec![
            row("dev", "default", true),
            row("feature/decode", "", false),
        ],
        hover: None,
        keys: vec![
            ("Tab", "next"),
            ("Enter", "use branch"),
            ("Ctrl+Shift+L", "link project"),
            ("Esc", "clear"),
        ],
        ..new_feature()
    }
}

/// The new feature dialog picking pull requests to review: two picked, the
/// same branch's suggested, one repository with none; Claude Code reviews.
fn new_review() -> super::NewFeatureView {
    let repo = |name: &str, place: &str, button: &str, touched| super::NewFeatureRepo {
        name: name.into(),
        place: place.into(),
        touched,
        fixed: false,
        base: Some(button.into()),
    };
    super::NewFeatureView {
        heading: "Review pull requests in \u{201c}aft\u{201d}".into(),
        title: Line::default(),
        hint: super::REVIEW_TITLE_HINT,
        root: "#31 of web picked too: same branch fix-login".into(),
        repos: vec![
            repo("api", "Fix the login redirect \u{b7} @bob", "#12", true),
            repo("shared", "no open pull request", "Pick\u{2026}", false),
            repo("web", "Login page shows the error \u{b7} @bob", "#31", true),
            repo("billing: ledger", "left out", "Pick\u{2026}", false),
        ],
        keys: vec![
            ("Tab", "next"),
            ("Space", "pick or leave out"),
            ("Alt+B", "pull request"),
            ("Ctrl+Shift+L", "link project"),
            ("Esc", "close"),
        ],
        reviewers: agent_chips(0, false),
        ..new_feature()
    }
}

/// The review dialog's list of a repository's open pull requests, the
/// keyboard on the one picked.
fn new_review_pulls() -> super::NewFeatureView {
    let row = |name: &str, place: &str, touched| super::NewFeatureRepo {
        name: name.into(),
        place: place.into(),
        touched,
        fixed: false,
        base: None,
    };
    super::NewFeatureView {
        title: Line::default(),
        hint: "Filter pull requests",
        focus: Some(0),
        choosing: true,
        root: "Open pull requests of web".into(),
        repos: vec![
            row(
                "#31 Login page shows the error",
                "@bob \u{b7} fix-login",
                true,
            ),
            row(
                "#29 Bump the date library",
                "@renovate \u{b7} renovate/date-fns \u{b7} draft",
                false,
            ),
        ],
        hover: None,
        keys: vec![
            ("Tab", "next"),
            ("Enter", "pick"),
            ("Ctrl+Shift+L", "link project"),
            ("Esc", "back"),
        ],
        ..new_review()
    }
}

/// A mark per stage, the first `done` of them done.
fn stage_marks(done: usize) -> Vec<NodeMark> {
    (0..crate::feature::Stage::ALL.len())
        .map(|i| match i {
            i if i < done => NodeMark::Done,
            i if i == done => NodeMark::Current,
            _ => NodeMark::Later,
        })
        .collect()
}

/// A feature's card at `stage`, the ones before it done.
fn card(stage: Stage, line: &str, tone: CardTone) -> FeatureCard {
    let done = Stage::ALL.iter().position(|s| *s == stage).unwrap_or(0);
    FeatureCard {
        marks: stage_marks(done),
        stage: Some(stage),
        line: line.into(),
        tone,
        gate: tone == CardTone::Waits,
        repos: 1,
    }
}

/// The sidebar with a feature's own workspace, its header showing the
/// feature's title and stage marks (the pointer on them), and another
/// feature with none open under the workspaces.
fn feature_workspace_sidebar() -> SidebarView {
    use super::FeatureRowView;
    let mut workspaces = workspaces();
    let feature = &mut workspaces[1];
    feature.name = "Bulk export of invoices".into();
    feature.feature = Some(FeatureCard {
        repos: 3,
        ..card(Stage::Verify, "Verify \u{b7} running", CardTone::Working)
    });
    SidebarView {
        workspaces,
        agents: agents(),
        active: Some(1),
        project: Some("aft".into()),
        header_lit: false,
        features: vec![FeatureRowView {
            title: "Faster search".into(),
            card: card(
                Stage::PlanReview,
                "Plan review \u{b7} 3d ago",
                CardTone::Idle,
            ),
        }],
        state: SidebarState {
            rows: vec![
                RowState::default(),
                RowState {
                    hover: 16,
                    stage: Some(3),
                    ..RowState::default()
                },
            ],
            ..SidebarState::default()
        },
    }
}

/// The sidebar with three open features under its workspaces, the second
/// under the pointer, the third turned back to Build as the rail draws it.
fn features_sidebar() -> SidebarView {
    use super::FeatureRowView;
    SidebarView {
        workspaces: workspaces(),
        agents: agents(),
        active: Some(0),
        project: Some("aft".into()),
        header_lit: false,
        features: vec![
            FeatureRowView {
                title: "Bulk export of invoices".into(),
                card: FeatureCard {
                    repos: 3,
                    ..card(Stage::Build, "Build \u{b7} milestone 2/5", CardTone::Waits)
                },
            },
            FeatureRowView {
                title: "A feature with a title far too long to fit".into(),
                card: card(
                    Stage::ManualTest,
                    "Manual test \u{b7} 1 of 12 passed, far too long",
                    CardTone::Failed,
                ),
            },
            FeatureRowView {
                title: "Turned back".into(),
                card: FeatureCard {
                    marks: Stage::ALL
                        .iter()
                        .map(|stage| match *stage {
                            Stage::Build => NodeMark::Waiting,
                            Stage::Verify => NodeMark::Ran,
                            Stage::AiReview => NodeMark::Paused,
                            s if s < Stage::Build => NodeMark::Done,
                            _ => NodeMark::Later,
                        })
                        .collect(),
                    ..card(
                        Stage::Build,
                        "Build \u{b7} waiting to send",
                        CardTone::Waits,
                    )
                },
            },
        ],
        state: SidebarState {
            features: vec![0, 16, 0],
            feature_dot: Some((1, 2)),
            ..SidebarState::default()
        },
    }
}

/// The features sidebar with its second workspace the active one: the
/// first one's blocked agent goes up into the "Needs you" list.
fn needs_you_sidebar() -> SidebarView {
    let mut view = SidebarView {
        active: Some(1),
        ..features_sidebar()
    };
    view.agents[0].iter_mut().for_each(|card| card.inbox = true);
    view
}

/// The sidebar of a project with no feature yet: the Features heading
/// offers to start one, the pointer on its "+".
fn no_features_sidebar() -> SidebarView {
    SidebarView {
        features: Vec::new(),
        state: SidebarState {
            features_plus: 16,
            ..SidebarState::default()
        },
        ..features_sidebar()
    }
}

const FEATURE_OUTPUT: (i32, i32) = (870, 720);
/// The sidebars' width: gerdr's default, the tightest fit a card has to
/// read in.
const SIDEBAR_W: i32 = super::SIDEBAR_WIDTH;

fn paint_features(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    let size = FEATURE_OUTPUT.into();
    vec![
        hash(painter.new_feature_dialog(&new_feature(), size, scale, p)),
        hash(painter.new_feature_dialog(&new_feature_bases(), size, scale, p)),
        hash(painter.sidebar_view(&features_sidebar(), (SIDEBAR_W, 520).into(), scale, p)),
        hash(painter.sidebar_view(
            &feature_workspace_sidebar(),
            (SIDEBAR_W, 520).into(),
            scale,
            p,
        )),
        hash(painter.sidebar_view(&needs_you_sidebar(), (SIDEBAR_W, 520).into(), scale, p)),
        hash(painter.sidebar_view(&no_features_sidebar(), (SIDEBAR_W, 520).into(), scale, p)),
        hash(painter.new_feature_dialog(&new_review(), size, scale, p)),
        hash(painter.new_feature_dialog(&new_review_pulls(), size, scale, p)),
    ]
}

const FEATURES: [&str; 8] = [
    "new_feature",
    "new_feature_bases",
    "sidebar_features",
    "sidebar_feature_workspace",
    "sidebar_needs_you",
    "sidebar_no_features",
    "new_review",
    "new_review_pulls",
];

/// The recorded hashes of `FEATURES`, per case in `CASES`.
const GOLDEN_FEATURES: [[u64; 8]; 4] = [
    // Dark at 1
    [
        0x9692ce9a85f8e589, // new_feature
        0x6715d713a5c98b6e, // new_feature_bases
        0x0b6a46bbcdcce91e, // sidebar_features
        0x8b774840526562fb, // sidebar_feature_workspace
        0x7ebf2b7305ef1e4d, // sidebar_needs_you
        0x4f6d2eef4e211ffa, // sidebar_no_features
        0x5fed715d8613d5cd, // new_review
        0x59e5bb70294102da, // new_review_pulls
    ],
    // Dark at 2
    [
        0x6956e487a7ff0ca8, // new_feature
        0xee82a764672d97e0, // new_feature_bases
        0x943fd9f0677017fb, // sidebar_features
        0x067c7417ffa1f415, // sidebar_feature_workspace
        0xda51fb9b77bbd23c, // sidebar_needs_you
        0xb65fa46d6c6f6a35, // sidebar_no_features
        0x2785b49aeee3d686, // new_review
        0x0ef33753f4289b15, // new_review_pulls
    ],
    // Light at 1
    [
        0xbc006152b9c2e6b5, // new_feature
        0x4c53b65ad1f57c15, // new_feature_bases
        0xbb6eb6babd58cb06, // sidebar_features
        0x774b2806fe493025, // sidebar_feature_workspace
        0xbbf93f1afaa0bf8c, // sidebar_needs_you
        0x91fd2bb7ab1b1020, // sidebar_no_features
        0x1088e5b1e3047420, // new_review
        0x131be4e70520445a, // new_review_pulls
    ],
    // Light at 2
    [
        0x7051a93823eaa6b3, // new_feature
        0xae890c1c909de326, // new_feature_bases
        0x489ae7a6ea3ff6f4, // sidebar_features
        0x7eec5630db219f44, // sidebar_feature_workspace
        0xdf473943fddc1570, // sidebar_needs_you
        0x493275c342a24108, // sidebar_no_features
        0x5e9fc7099c68a2ed, // new_review
        0xfc15be0c278b67c4, // new_review_pulls
    ],
];

#[test]
fn the_feature_surfaces_paint_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_FEATURES.iter().map(|row| row.as_slice()).collect();
    check(&FEATURES, &golden, paint_features);
}

/// With `GERDR_DUMP_DIR` set, write the feature surfaces there.
#[test]
fn dump_features() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    let size = FEATURE_OUTPUT.into();
    let shadow = 2 * super::SHADOW;
    for (theme, scale) in CASES {
        let p = theme.palette();
        let px = |v: i32| (v as f64 * scale).round() as usize;
        let tag = format!("{}_{}", theme.name(), scale);
        let dialog = super::new_feature_layout(size, 3, false, &[]).panel;
        dump(
            &dir,
            &format!("new_feature_{tag}"),
            painter.new_feature_dialog(&new_feature(), size, scale, p),
            px(dialog.size.w + shadow),
        );
        dump(
            &dir,
            &format!("new_feature_bases_{tag}"),
            painter.new_feature_dialog(&new_feature_bases(), size, scale, p),
            px(dialog.size.w + shadow),
        );
        for (name, view) in [
            ("new_review", new_review()),
            ("new_review_pulls", new_review_pulls()),
        ] {
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.new_feature_dialog(&view, size, scale, p),
                px(dialog.size.w + shadow),
            );
        }
        dump(
            &dir,
            &format!("sidebar_features_{tag}"),
            painter.sidebar_view(&features_sidebar(), (SIDEBAR_W, 520).into(), scale, p),
            px(SIDEBAR_W),
        );
        dump(
            &dir,
            &format!("sidebar_no_features_{tag}"),
            painter.sidebar_view(&no_features_sidebar(), (SIDEBAR_W, 520).into(), scale, p),
            px(SIDEBAR_W),
        );
        dump(
            &dir,
            &format!("sidebar_feature_workspace_{tag}"),
            painter.sidebar_view(
                &feature_workspace_sidebar(),
                (SIDEBAR_W, 520).into(),
                scale,
                p,
            ),
            px(SIDEBAR_W),
        );
        dump(
            &dir,
            &format!("sidebar_needs_you_{tag}"),
            painter.sidebar_view(&needs_you_sidebar(), (SIDEBAR_W, 520).into(), scale, p),
            px(SIDEBAR_W),
        );
    }
}

/// The git identity dialog: filled from the GitHub account, the pointer
/// on Save and retry; else (`error`) a bad email said, the keyboard on
/// the scope, kept for the failed repository only.
fn identity(error: bool) -> super::IdentityView {
    super::IdentityView {
        body: "They go on every commit. Once saved, Commit and push runs again.".into(),
        name: Line::from("Octo Cat"),
        email: Line::from(if error {
            "octo cat"
        } else {
            "42+octo@users.noreply.github.com"
        }),
        focus: if error {
            super::IdentityFocus::Scope
        } else {
            super::IdentityFocus::Email
        },
        global: !error,
        only: "Only assettag-api".into(),
        error: error.then(|| "That email doesn't look right: it needs an @ and no spaces.".into()),
        hover: (!error).then_some(super::IdentityHit::Button(1)),
        keys: if error {
            vec![("Tab", "next"), ("Space", "switch"), ("Esc", "close")]
        } else {
            vec![("Tab", "next"), ("Esc", "close")]
        },
    }
}

const IDENTITY_OUTPUT: (i32, i32) = (870, 720);

fn paint_identity(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    let size = IDENTITY_OUTPUT.into();
    vec![
        hash(painter.identity_dialog(&identity(false), size, scale, p)),
        hash(painter.identity_dialog(&identity(true), size, scale, p)),
    ]
}

const IDENTITY: [&str; 2] = ["identity", "identity_error"];

/// The recorded hashes of `IDENTITY`, per case in `CASES`.
const GOLDEN_IDENTITY: [[u64; 2]; 4] = [
    // Dark at 1
    [
        0x66ed7039660e086e, // identity
        0xd70c282d357e4e57, // identity_error
    ],
    // Dark at 2
    [
        0x8ae8b2c08d3bffa7, // identity
        0xcc408c57e1e69d0a, // identity_error
    ],
    // Light at 1
    [
        0x78bb1cfef6b30521, // identity
        0xafa09a2a69722e62, // identity_error
    ],
    // Light at 2
    [
        0x6ef5362f4d308ce6, // identity
        0x82ad02872b871b07, // identity_error
    ],
];

#[test]
fn the_identity_dialog_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_IDENTITY.iter().map(|row| row.as_slice()).collect();
    check(&IDENTITY, &golden, paint_identity);
}

/// With `GERDR_DUMP_DIR` set, write the identity dialog there.
#[test]
fn dump_identity() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    let size = IDENTITY_OUTPUT.into();
    for (theme, scale) in CASES {
        let p = theme.palette();
        let px = |v: i32| (v as f64 * scale).round() as usize;
        let tag = format!("{}_{}", theme.name(), scale);
        for (name, error) in [("identity", false), ("identity_error", true)] {
            let panel = super::identity_layout(size, error).panel;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.identity_dialog(&identity(error), size, scale, p),
                px(panel.size.w + 2 * super::SHADOW),
            );
        }
    }
}

/// With `GERDR_DUMP_DIR` set, write the project menu there: as in the
/// golden surfaces, and with the selected project's id being changed.
#[test]
fn dump_project_menu() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    let size = (800, 600).into();
    let mut changing = project_menu();
    if let Some(MenuEntry::Project { renaming, .. }) = changing.entries.get_mut(3) {
        *renaming = Some(crate::text_field::TextField::from("gerdr-app").line());
    }
    changing.hints = vec![
        ("Enter".into(), "change id".into()),
        ("Esc".into(), "cancel".into()),
    ];
    for (theme, scale) in CASES {
        let p = theme.palette();
        let px = |v: i32| (v as f64 * scale).round() as usize;
        let tag = format!("{}_{}", theme.name(), scale);
        for (name, view) in [
            ("project_menu", project_menu()),
            ("project_menu_id", changing.clone()),
        ] {
            let width = super::project_menu_layout(&view, size).panel.size.w + 2 * super::SHADOW;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.project_menu(&view, size, scale, p),
                px(width),
            );
        }
    }
}

// ------------------------------------------------------- connect a machine

/// The dialog connecting a machine: empty, connecting, and after a failure
/// with what to do about it.
fn machine(case: usize) -> super::MachineView {
    use crate::text_field::TextField;
    let mut target = TextField::default();
    if case > 0 {
        target.set("ana@build-box");
    }
    let (body, error, how, buttons, keys): (&str, _, _, &[&str], Vec<_>) = match case {
        0 => (
            "Agents and checks can run there. It uses your SSH keys.",
            None,
            None,
            &["Cancel", "Connect"],
            vec![("Tab", "next"), ("Esc", "close")],
        ),
        1 => (
            "herdr 0.9.5, starting its server\u{2026}",
            None,
            None,
            &["Stop"],
            vec![("Esc", "stop")],
        ),
        _ => (
            "Agents and checks can run there. It uses your SSH keys.",
            Some("It didn't accept any of your SSH keys.".to_owned()),
            Some(
                "Add a key with ssh-copy-id ana@build-box. A locked key needs ssh-add.".to_owned(),
            ),
            &["Cancel", "Copy command", "Try again"],
            vec![("Tab", "next"), ("Alt+X", "fix"), ("Esc", "close")],
        ),
    };
    super::MachineView {
        body: body.into(),
        target: target.line(),
        name: TextField::default().line(),
        name_hint: if case > 0 {
            "build-box".into()
        } else {
            String::new()
        },
        focus: super::MachineFocus::Target,
        error,
        how,
        buttons: buttons.iter().map(|b| (*b).to_owned()).collect(),
        busy: case == 1,
        hover: (case == 0).then_some(super::MachineHit::Button(1)),
        keys,
    }
}

const MACHINE: [&str; 3] = ["machine", "machine_connecting", "machine_failed"];

fn paint_machine(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    let size = IDENTITY_OUTPUT.into();
    (0..MACHINE.len())
        .map(|case| hash(painter.machine_dialog(&machine(case), size, scale, p)))
        .collect()
}

/// The recorded hashes of `MACHINE`, per case in `CASES`.
const GOLDEN_MACHINE: [[u64; 3]; 4] = [
    // Dark at 1
    [
        0x57d4ee0d9167bfe5, // machine
        0xe33b4450878421fe, // machine_connecting
        0xa048f23b72101f43, // machine_failed
    ],
    // Dark at 2
    [
        0x9fc228ef547aeef7, // machine
        0x8b9925e3757680a9, // machine_connecting
        0xd26bfde4dce393ff, // machine_failed
    ],
    // Light at 1
    [
        0x06a0197da4e50024, // machine
        0x5040c9498ae350d1, // machine_connecting
        0xd5be23c620e31865, // machine_failed
    ],
    // Light at 2
    [
        0xb606aa78a50d9de0, // machine
        0xa3c9c9f1b9aa03e4, // machine_connecting
        0x8126f0146a8d70d0, // machine_failed
    ],
];

#[test]
fn the_machine_dialog_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_MACHINE.iter().map(|row| row.as_slice()).collect();
    check(&MACHINE, &golden, paint_machine);
}

/// With `GERDR_DUMP_DIR` set, write the dialog connecting a machine there.
#[test]
fn dump_machine() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    let size = IDENTITY_OUTPUT.into();
    for (theme, scale) in CASES {
        let p = theme.palette();
        let px = |v: i32| (v as f64 * scale).round() as usize;
        let tag = format!("{}_{}", theme.name(), scale);
        for (case, name) in MACHINE.iter().enumerate() {
            let view = machine(case);
            let panel = super::machine_layout(&view, size).panel;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.machine_dialog(&view, size, scale, p),
                px(panel.size.w + 2 * super::SHADOW),
            );
        }
    }
}

// ------------------------------------------------------- machines panel

/// The machines panel: a connected machine with three runs, the keyboard
/// on a run's button; a failed one with what to do and its fix; the add
/// row; a machine Del was pressed on once; then the connected one's edit
/// form: open with the keyboard in its address, trying the new address,
/// the address reaching another machine, and two addresses with the
/// keyboard in the second and the pointer on its ×; then the connected one
/// found to be the same machine as another saved one.
fn machines_panel(case: usize) -> super::MachinesPanelView {
    use super::{Dot, MachineDetailView, MachineEditView, MachineRowView, MachineRunView};
    use super::{MachinesFocus, MachinesHit, MachinesPanelView};
    let strings = |xs: &[&str]| xs.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    let failed = case == 1;
    let row = |name: &str, address: &str, state: &str, dot, armed| MachineRowView {
        name: name.into(),
        address: address.into(),
        state: state.into(),
        dot,
        armed,
    };
    let mut rows = vec![
        row(
            "ZenBook",
            "lga@zenbook-office",
            "connected",
            Dot::Done,
            false,
        ),
        row(
            "Build box",
            "ana@build-box",
            if failed { "failed" } else { "off" },
            if failed { Dot::Failed } else { Dot::Idle },
            case == 3,
        ),
    ];
    let run = |title: &str, detail: &str, dot, buttons: &[&str]| MachineRunView {
        title: title.into(),
        detail: detail.into(),
        dot,
        buttons: strings(buttons),
    };
    let zenbook = MachineDetailView {
        name: "ZenBook".into(),
        dot: Dot::Done,
        facts: strings(&[
            "lga@zenbook-office",
            "up 2h 5m \u{b7} herdr 0.9.3 \u{b7} load 1.2 / 8",
            "id 4f2a9c",
        ]),
        problem: None,
        how: None,
        runs: vec![
            run(
                "asset-tag \u{b7} Build agent",
                "Claude \u{b7} your turn \u{b7} 12m",
                Dot::Blocked,
                &["Show", "Stop", "Open feature"],
            ),
            run(
                "asset-tag \u{b7} Verify checks",
                "3 of 5 checks passed \u{b7} 4m",
                Dot::Working,
                &["Show", "Open feature"],
            ),
            run(
                "asset-tag \u{b7} sandbox",
                "running \u{b7} 40m",
                Dot::Done,
                &["Show", "Stop", "Open feature"],
            ),
        ],
        no_runs: "Nothing runs there".into(),
        buttons: strings(&["Remove", "Edit", "Disconnect"]),
        edit: None,
    };
    let build_box = MachineDetailView {
        name: "Build box".into(),
        dot: if failed { Dot::Failed } else { Dot::Idle },
        facts: strings(&["ana@build-box"]),
        problem: failed.then(|| "It didn't accept any of your SSH keys.".to_owned()),
        how: failed.then(|| {
            "Add a key with ssh-copy-id ana@build-box. A locked key needs ssh-add.".to_owned()
        }),
        runs: Vec::new(),
        no_runs: "Connect it to see what runs there".into(),
        buttons: if failed {
            strings(&["Remove", "Edit", "Copy command", "Connect"])
        } else {
            strings(&["Remove", "Edit", "Connect"])
        },
        edit: None,
    };
    let (selected, detail, focus, hover) = match case {
        0 => (
            0,
            Some(zenbook),
            MachinesFocus::Run(0, 1),
            Some(MachinesHit::Row(1)),
        ),
        1 => (
            1,
            Some(build_box),
            MachinesFocus::List,
            Some(MachinesHit::Button(2)),
        ),
        2 => (2, None, MachinesFocus::List, Some(MachinesHit::Hint(0))),
        3 => (1, Some(build_box), MachinesFocus::List, None),
        8 => {
            rows.push(row(
                "ZenBook Office",
                "lga-asset@zenbook-office",
                "connected",
                Dot::Done,
                false,
            ));
            let mut zenbook = zenbook;
            zenbook.facts.push("Same machine as ZenBook Office".into());
            zenbook.buttons = strings(&["Merge ZenBook Office", "Remove", "Edit", "Disconnect"]);
            (0, Some(zenbook), MachinesFocus::List, None)
        }
        _ => {
            let (busy, failed) = (case == 5, case == 6);
            let addresses = if case == 7 {
                vec![
                    Line::from("lga-asset@zenbook-office"),
                    Line::from("lga-asset@192.168.101.14"),
                ]
            } else {
                vec![Line::from("lga@zenbook-home")]
            };
            let edit = MachineEditView {
                name: Line::from("ZenBook"),
                addresses,
                error: failed
                    .then(|| "That address reaches another machine, not ZenBook".to_owned()),
                how: failed.then(|| {
                    "It calls itself fedora-home. Check the address, or save it anyway.".to_owned()
                }),
                progress: busy.then(|| "SSH reached, looking around\u{2026}".to_owned()),
                buttons: if busy {
                    strings(&["Stop"])
                } else if failed {
                    strings(&["Cancel", "Save anyway", "Try again"])
                } else {
                    strings(&["Cancel", "Save"])
                },
                busy,
            };
            let mut zenbook = zenbook;
            zenbook.runs.truncate(1);
            zenbook.edit = Some(edit);
            let (focus, hover) = match case {
                4 => (
                    MachinesFocus::EditAddress(0),
                    Some(MachinesHit::EditButton(0)),
                ),
                5 => (MachinesFocus::EditAddress(0), None),
                7 => (
                    MachinesFocus::EditAddress(1),
                    Some(MachinesHit::EditRemoveAddress(1)),
                ),
                _ => (MachinesFocus::EditButton(2), None),
            };
            (0, Some(zenbook), focus, hover)
        }
    };
    MachinesPanelView {
        tab: crate::chrome::MachinesTab::Machines,
        rows,
        selected,
        first: 0,
        detail,
        focus,
        runs_first: 0,
        hover,
        hints: [
            ("Up / Down", "move"),
            ("Tab", "next button"),
            ("Enter", "press"),
            ("Del", "remove"),
            ("Esc", "close"),
        ]
        .map(|(k, a)| (k.to_owned(), a.to_owned()))
        .into(),
    }
}

const MACHINES_PANEL: [&str; 10] = [
    "machines_connected",
    "machines_failed",
    "machines_add",
    "machines_armed",
    "machines_edit",
    "machines_edit_trying",
    "machines_edit_failed",
    "machines_edit_addresses",
    "machines_same_machine",
    "machines_placement",
];

/// The Placement page: the open project's stages, the second focused, the
/// pointer over the first's button.
fn machines_placement() -> super::MachinesPanelView {
    use super::MachinesPanelView;
    use super::{Dot, MachineDetailView, MachineRunView, MachinesFocus, MachinesHit};
    let stage = |title: &str, detail: &str, dot, button: &str| MachineRunView {
        title: title.into(),
        detail: detail.into(),
        dot,
        buttons: vec![button.into()],
    };
    MachinesPanelView {
        tab: crate::chrome::MachinesTab::Placement,
        rows: Vec::new(),
        selected: 0,
        first: 0,
        detail: Some(MachineDetailView {
            name: "asset-tag".into(),
            dot: Dot::Idle,
            facts: vec![
                "Where each stage runs when a feature doesn't choose its own (Ctrl+Shift+W on its page)"
                    .into(),
            ],
            problem: None,
            how: None,
            runs: vec![
                stage("Discover", "Every feature follows this", Dot::Done, "Runs here"),
                stage("Build", "2 features choose their own", Dot::Done, "Runs on ZenBook"),
                stage("Verify", "Every feature follows this", Dot::Done, "Least busy"),
                stage("Manual test", "1 feature chooses its own", Dot::Blocked, "Runs on Build box"),
            ],
            no_runs: String::new(),
            buttons: Vec::new(),
            edit: None,
        }),
        focus: MachinesFocus::Run(1, 0),
        runs_first: 0,
        hover: Some(MachinesHit::RunButton(0, 0)),
        hints: [
            ("Up / Down", "move"),
            ("Left / Right", "machine"),
            ("Ctrl+PgUp", "machines"),
            ("Esc", "close"),
        ]
        .map(|(k, a)| (k.to_owned(), a.to_owned()))
        .into(),
    }
}

/// Case `case` of `MACHINES_PANEL`.
fn machines_case(case: usize) -> super::MachinesPanelView {
    match case {
        9 => machines_placement(),
        _ => machines_panel(case),
    }
}

fn paint_machines_panel(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    let size = IDENTITY_OUTPUT.into();
    (0..MACHINES_PANEL.len())
        .map(|case| hash(painter.machines_panel(&machines_case(case), size, scale, p)))
        .collect()
}

/// The recorded hashes of `MACHINES_PANEL`, per case in `CASES`.
const GOLDEN_MACHINES_PANEL: [[u64; 10]; 4] = [
    // Dark at 1
    [
        0xd2c1e8fc4b71df11, // machines_connected
        0x0920e121470732fb, // machines_failed
        0xa1653e4247588191, // machines_add
        0x4fa5080c59f77865, // machines_armed
        0x47e461117bd3bd93, // machines_edit
        0x1452d0e2fdc70617, // machines_edit_trying
        0x2387b80acb09efb0, // machines_edit_failed
        0xffe46a0c8e055860, // machines_edit_addresses
        0xaf09c8ce8bd95c1e, // machines_same_machine
        0xa2d4fe07a98f5057, // machines_placement
    ],
    // Dark at 2
    [
        0xe3649a9c3eab5caa, // machines_connected
        0x67f36bbe8435743a, // machines_failed
        0x82a37c986e4beeb4, // machines_add
        0x5670f9fbaa4690fe, // machines_armed
        0xb0f018e218789b40, // machines_edit
        0x6d870c0326a0b637, // machines_edit_trying
        0x5e948d62a46fb64b, // machines_edit_failed
        0x6ae9ed2892b7482b, // machines_edit_addresses
        0x9e3485b612d4b0c2, // machines_same_machine
        0xb8eae5c6ba8119a5, // machines_placement
    ],
    // Light at 1
    [
        0x0bb8eb61bff96710, // machines_connected
        0x060f30ab83bdbfd3, // machines_failed
        0x875a2f388dea3bee, // machines_add
        0xeee43058ca9c5ba3, // machines_armed
        0xe0a457103e9ea811, // machines_edit
        0x6e73d762d64f7191, // machines_edit_trying
        0x8622acbaa384a57e, // machines_edit_failed
        0x7cf39bcbbcf1c0b2, // machines_edit_addresses
        0xc2d4b746355a474f, // machines_same_machine
        0x393dbfdd40ad5829, // machines_placement
    ],
    // Light at 2
    [
        0x7e50298e617e3e79, // machines_connected
        0xa1d59246dfe534f8, // machines_failed
        0x3b1287d6adfe64af, // machines_add
        0xce0c191794c850f3, // machines_armed
        0x37b47e8347a04c53, // machines_edit
        0x0694dd6746425814, // machines_edit_trying
        0x428d7930bf9fbddb, // machines_edit_failed
        0x7a926d591e9905ba, // machines_edit_addresses
        0xce6fd0642babaa03, // machines_same_machine
        0x55856f6fa4090cd8, // machines_placement
    ],
];

#[test]
fn the_machines_panel_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_MACHINES_PANEL
        .iter()
        .map(|row| row.as_slice())
        .collect();
    check(&MACHINES_PANEL, &golden, paint_machines_panel);
}

/// With `GERDR_DUMP_DIR` set, write the machines panel there.
#[test]
fn dump_machines_panel() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    let size = IDENTITY_OUTPUT.into();
    for (theme, scale) in CASES {
        let p = theme.palette();
        let px = |v: i32| (v as f64 * scale).round() as usize;
        let tag = format!("{}_{}", theme.name(), scale);
        for (case, name) in MACHINES_PANEL.iter().enumerate() {
            let view = machines_case(case);
            let panel = super::machines_layout(&view, size).panel;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.machines_panel(&view, size, scale, p),
                px(panel.size.w + 2 * super::SHADOW),
            );
        }
    }
}

// ------------------------------------------------------- restart dialog

/// The restart dialog: a workspace's commands, one exited and one unchecked,
/// and what a feature's sandbox would run, the cursor on one to leave out.
fn restart(sandbox: bool) -> super::RestartView {
    let row = |label: &str, place: &str, exited: bool, checked: bool| super::RestartRow {
        label: label.into(),
        place: place.into(),
        exited,
        checked,
    };
    match sandbox {
        false => super::RestartView {
            title: "Restart which commands in \u{201c}api\u{201d}?".into(),
            body: "The checked ones stop and run again; a new command is asked about",
            action: "Restart",
            rows: vec![
                row("runserver", "~/work/api", false, true),
                row("celery worker", "~/work/api", true, true),
                row("pytest", "~/work/api", false, false),
            ],
            cursor: 0,
            top: 0,
            selected: 1,
            hover: None,
        },
        true => super::RestartView {
            title: "Run which commands in this sandbox?".into(),
            body: "The checked ones run inside it; this feature keeps the choice",
            action: "Start",
            rows: vec![
                row(
                    "python manage.py migrate",
                    "backend \u{b7} runs first",
                    false,
                    true,
                ),
                row("python manage.py runserver 9000", "backend", false, true),
                row("celery -A app worker -l INFO", "backend", false, true),
                row("celery -A app beat -l INFO", "backend", false, false),
                row("nx serve", "frontend \u{b7} from main", false, true),
            ],
            cursor: 3,
            top: 0,
            selected: 1,
            hover: None,
        },
    }
}

/// What the restart dialog is painted on.
const RESTART_OUTPUT: (i32, i32) = (1280, 720);

fn paint_restart(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    let size = RESTART_OUTPUT.into();
    vec![
        hash(painter.restart_dialog(&restart(false), size, scale, p)),
        hash(painter.restart_dialog(&restart(true), size, scale, p)),
    ]
}

/// The surfaces `paint_restart` hashes, in order.
const RESTART: [&str; 2] = ["restart", "restart_sandbox"];

/// The recorded hashes of `RESTART`, per case in `CASES`.
const GOLDEN_RESTART: [[u64; 2]; 4] = [
    // Dark at 1
    [
        0xa63057d5848117ec, // restart
        0xa74d8ba5cb53e20c, // restart_sandbox
    ],
    // Dark at 2
    [
        0x68d6670003a64230, // restart
        0xfb62a3dc040a7c32, // restart_sandbox
    ],
    // Light at 1
    [
        0x250951f4e62b011c, // restart
        0x64306513fa80a77d, // restart_sandbox
    ],
    // Light at 2
    [
        0x4c437cf00a305c62, // restart
        0x2dd4fbb35b173ee8, // restart_sandbox
    ],
];

#[test]
fn the_restart_dialog_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_RESTART.iter().map(|row| row.as_slice()).collect();
    check(&RESTART, &golden, paint_restart);
}

/// With `GERDR_DUMP_DIR` set, write the restart dialog there.
#[test]
fn dump_restart() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    let size = RESTART_OUTPUT.into();
    for (theme, scale) in CASES {
        let p = theme.palette();
        let px = |v: i32| (v as f64 * scale).round() as usize;
        let tag = format!("{}_{}", theme.name(), scale);
        for (name, sandbox) in [("restart", false), ("restart_sandbox", true)] {
            let view = restart(sandbox);
            let panel = super::restart_layout(size, view.rows.len(), view.action).panel;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.restart_dialog(&view, size, scale, p),
                px(panel.size.w + 2 * super::SHADOW),
            );
        }
    }
}

// ------------------------------------------------------- project commands

/// The project commands dialog, as `case` says: its Checks with a
/// suggestion and several lines, Setup with two projects, a form error with
/// the file edited outside gerdr, and on a low output scrolled.
fn commands(case: usize) -> super::CommandsView {
    let row =
        |label: &str, tag: Option<&str>, suggestion: Option<&str>, text: &str| super::CommandsRow {
            heading: None,
            label: label.into(),
            tag: tag.map(str::to_owned),
            suggestion: suggestion.map(str::to_owned),
            borrowed: Vec::new(),
            line: text.into(),
        };
    let mut view = super::CommandsView {
        tabs: &super::COMMANDS_PROJECT_TABS,
        tab: super::CommandsTab::Checks,
        body: "One per line, at once; in turn within a folder (&& keeps an order). Changing code fails Verify.",
        note: None,
        rows: vec![
            row(
                "api",
                Some("in this feature"),
                None,
                "cargo test\ncargo clippy -- -D warnings",
            ),
            row("web", None, Some("npm test"), ""),
            row("old", Some("not found"), None, "make check"),
            row("Whole feature", Some("in the feature folder"), None, ""),
        ],
        limit: Some("1200".into()),
        focus: super::CommandsFocus::Row(0),
        down: 0,
        error: None,
        stale: false,
        hover: None,
        keys: vec![
            ("Shift+Enter", "new line"),
            ("Esc", "close"),
            ("Tab", "next"),
        ],
    };
    match case {
        1 => {
            view.tab = super::CommandsTab::Setup;
            view.body = "One command per line. They run in a new worktree, once it has its files.";
            view.note = Some(("finding repositories\u{2026}".into(), false));
            view.rows = vec![
                row("api", None, None, ""),
                row("site", None, Some("npm ci"), ""),
            ];
            view.rows[0].heading = Some("Home".into());
            view.rows[1].heading = Some("Linked".into());
            view.limit = None;
            view.hover = Some(super::CommandsHit::Use(1));
        }
        2 => {
            view.note = Some(("Couldn't look for repositories: no git".into(), true));
            view.error =
                Some("The project file was edited outside gerdr. Reload it to save here.".into());
            view.stale = true;
            view.focus = super::CommandsFocus::Limit;
            view.hover = Some(super::CommandsHit::Button(1));
        }
        3 => view.down = 60,
        4 => {
            view.tab = super::CommandsTab::Run;
            view.body = "One command per line, each in a terminal of the sandbox. A line starting with # doesn't run.";
            view.rows = vec![
                row(
                    "aft-backend",
                    Some("in this feature"),
                    None,
                    "./runlocal.sh\n./run_celery.sh\n# ./run_celery_beat.sh",
                ),
                row("web", None, Some("npx nx run ao-app:serve:local"), ""),
            ];
            view.limit = None;
        }
        5 => {
            view.tabs = &super::COMMANDS_FEATURE_TABS;
            view.tab = super::CommandsTab::Feature;
            view.body = crate::project_commands::FEATURE_BODY;
            view.rows = vec![
                row("api", None, None, "./e2e.sh login"),
                row("web", Some("runs once this feature changes it"), None, ""),
                row("Whole feature", Some("in the feature folder"), None, ""),
            ];
            let borrowed = |command: &str, skipped| super::CommandsBorrowed {
                command: command.into(),
                skipped,
            };
            view.rows[0].borrowed = vec![
                borrowed("cargo test", false),
                borrowed("cargo clippy -- -D warnings", true),
            ];
            view.rows[1].borrowed = vec![borrowed("npm test", false)];
            view.limit = None;
            view.hover = Some(super::CommandsHit::Tab(super::CommandsTab::Checks));
        }
        6 => {
            view.tabs = &super::COMMANDS_FEATURE_TABS;
            view.tab = super::CommandsTab::Pages;
            view.body = crate::project_commands::PAGES_BODY;
            view.rows = vec![row(
                "Sandbox app",
                Some("listening now: 4200, 8000, 8001"),
                Some("App http://localhost/, Web app http://localhost:8000/"),
                "",
            )];
            view.limit = None;
            view.keys.push(("Alt+U", "use"));
        }
        _ => {}
    }
    view
}

/// The output each of `COMMANDS` is painted on.
fn commands_output(case: usize) -> (i32, i32) {
    match case {
        3 => (640, 420),
        _ => (870, 720),
    }
}

fn paint_commands(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    (0..COMMANDS.len())
        .map(|case| {
            let size = commands_output(case).into();
            hash(painter.commands_dialog(&commands(case), size, scale, p))
        })
        .collect()
}

const COMMANDS: [&str; 7] = [
    "commands",
    "commands_setup",
    "commands_stale",
    "commands_low",
    "commands_run",
    "commands_feature",
    "commands_pages",
];

/// The recorded hashes of `COMMANDS`, per case in `CASES`.
const GOLDEN_COMMANDS: [[u64; 7]; 4] = [
    // Dark at 1
    [
        0x4b68c279b631247f, // commands
        0xb9f2d3709ab1a448, // commands_setup
        0x5dae424212e9c9eb, // commands_stale
        0xbbd1ece890b082e1, // commands_low
        0xadd8f1af46928d65, // commands_run
        0xf6f444832f2f8658, // commands_feature
        0xbf383635656b6196, // commands_pages
    ],
    // Dark at 2
    [
        0x1a4c263938e1e902, // commands
        0xcbd90d77ed10cb21, // commands_setup
        0xf6a012a46d4a09a2, // commands_stale
        0x769895bfc9759c43, // commands_low
        0xc1d2681163b7f6af, // commands_run
        0x8c107e4f27f49308, // commands_feature
        0xf71b979a00d2a843, // commands_pages
    ],
    // Light at 1
    [
        0xd79a0c52c4e8f551, // commands
        0xdf74ffecbcdd81b4, // commands_setup
        0xd0e3b25224c7f31c, // commands_stale
        0xe0b8bc74a69b0ae1, // commands_low
        0x895411f814c3b6e4, // commands_run
        0x9fdefc66d7c4bcf6, // commands_feature
        0xdd98c7c18dade87e, // commands_pages
    ],
    // Light at 2
    [
        0x80f47cd4612cc5d4, // commands
        0xf4fd6315979622a4, // commands_setup
        0x22aae9fa0b3c428f, // commands_stale
        0xab9b42508090f326, // commands_low
        0x7777f33e37d1159f, // commands_run
        0xa691e55b5a165950, // commands_feature
        0x4f86e5baaae71998, // commands_pages
    ],
];

#[test]
fn the_commands_dialog_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_COMMANDS.iter().map(|row| row.as_slice()).collect();
    check(&COMMANDS, &golden, paint_commands);
}

/// With `GERDR_DUMP_DIR` set, write the project commands dialog there.
#[test]
fn dump_commands() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    for (theme, scale) in CASES {
        let p = theme.palette();
        let px = |v: i32| (v as f64 * scale).round() as usize;
        let tag = format!("{}_{}", theme.name(), scale);
        for (case, name) in COMMANDS.iter().enumerate() {
            let size = commands_output(case).into();
            let view = commands(case);
            let panel = super::commands_layout(&view, size).panel;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.commands_dialog(&view, size, scale, p),
                px(panel.size.w + 2 * super::SHADOW),
            );
        }
    }
}

// ------------------------------------------------------------ feature tab

/// How high the feature tab is painted, and the widths it is painted at:
/// the chips row under 870, no activity column under 1180, and all three
/// columns.
const TAB_HEIGHT: i32 = 900;
const TAB_WIDTHS: [i32; 5] = [500, 850, 860, 1180, 1920];

fn tab_line(text: &str, tone: super::CheckTone) -> super::CheckLine {
    super::CheckLine {
        text: text.into(),
        tone,
    }
}

/// A feature's tab with Build current and running: claude at work with
/// the last three lines of its pane, three of five milestones done by
/// name, three repositories (shared not touched), the two touched with
/// the lines they change.
pub(super) fn feature_tab() -> super::FeatureTabView {
    use super::{
        ActivityRow, ButtonStyle, CheckTone, Dot, FeatureRepoRow, FeatureTabView, NodeMark,
        PageButton, PageSection, RailNode, StagePage, TabArea,
    };
    use crate::{agent_icons::AgentIcon, feature::Stage};
    let rail = Stage::ALL
        .iter()
        .enumerate()
        .map(|(i, stage)| RailNode {
            label: stage.label().into(),
            line: match i {
                0 => "2h ago".into(),
                1 => "1h ago".into(),
                2 => "skipped".into(),
                3 => "working 14m".into(),
                _ => String::new(),
            },
            mark: match i {
                0..=2 => NodeMark::Done,
                3 => NodeMark::Current,
                _ => NodeMark::Later,
            },
            selected: i == 3,
            agent: match i {
                0 => Some((AgentIcon::Cursor, "grok-4.7".into())),
                1 => Some((
                    AgentIcon::Claude,
                    "a-model-name-too-wide-for-its-slot-here".into(),
                )),
                _ => None,
            },
        })
        .collect();
    let stat = |files, added, removed| Stat {
        files,
        added,
        removed,
    };
    let repo = |name: &str,
                line: &str,
                touched: bool,
                button: Option<&'static str>,
                stat: Option<Stat>| FeatureRepoRow {
        name: name.into(),
        line: line.into(),
        kind: match touched {
            true => super::RepoKind::Active,
            false => super::RepoKind::Context,
        },
        group: None,
        busy: false,
        buttons: button.into_iter().collect(),
        remove: true,
        stat,
    };
    let activity = [
        ("Build started by you", "14m ago", CheckTone::Note),
        ("plan.md approved", "1h ago", CheckTone::Ready),
        (
            "brief.md reviewed, with two comments sent to the agent",
            "2h ago",
            CheckTone::Ready,
        ),
        (
            "Verify failed and sent it back to Build (run / 21)",
            "3h ago",
            CheckTone::Blocker,
        ),
        ("Feature created", "3h ago", CheckTone::Note),
    ]
    .map(|(what, ago, tone)| ActivityRow {
        what: what.into(),
        ago: ago.into(),
        tone,
    });
    let hints = [
        ("Left / Right", "stage"),
        ("Enter", "mark done"),
        ("Tab", "repositories"),
        ("F1", "keys"),
    ];
    FeatureTabView {
        review_page: false,
        review_switch: None,
        test_strip: None,
        review_keys: false,
        title: "Bulk export of invoices".into(),
        sub: "bulk-export \u{b7} ~/Work/aft/.features/bulk-export \u{b7} started 3h ago".into(),
        which: Some((0, 2)),
        remove: "Abandon\u{2026}",
        rail,
        back: None,
        repos: vec![
            repo(
                "api",
                "feature/bulk-export",
                true,
                None,
                Some(stat(14, 212, 38)),
            ),
            repo(
                "web",
                "feature/bulk-export",
                true,
                None,
                Some(stat(1, 96, 4)),
            ),
            repo("shared", "context at dev", false, Some("Feature"), None),
        ],
        repos_error: None,
        repo: 0,
        focus: TabArea::Stages,
        expanded: false,
        more: 2,
        picked: None,
        repo_notes: Vec::new(),
        page: StagePage {
            primary: None,
            menu: Vec::new(),
            title: "Build".into(),
            about: Stage::Build.about().into(),
            status: Some(tab_line("claude is working in tab 2", CheckTone::Note)),
            buttons: vec![
                PageButton {
                    label: "Mark done".into(),
                    style: ButtonStyle::Primary,
                },
                PageButton {
                    label: "Show agent".into(),
                    style: ButtonStyle::Plain,
                },
                PageButton {
                    label: "Restart".into(),
                    style: ButtonStyle::Quiet,
                },
            ],
            sections: vec![
                PageSection::Agent {
                    stage: crate::feature::Stage::Build,
                    icon: AgentIcon::Claude,
                    name: "claude".into(),
                    dot: Dot::Working,
                    status: "working \u{b7} tab 2".into(),
                    tail: vec![
                        "\u{203a} Editing api/src/export/csv.rs".into(),
                        "\u{203a} cargo test export:: (12 passed)".into(),
                        "\u{203a} Reading web/src/export/Dialog.tsx".into(),
                    ],
                    buttons: vec![1, 2],
                    picks: Vec::new(),
                    model: None,
                    waits: false,
                },
                PageSection::Lines {
                    label: "Fix round".into(),
                    lines: Vec::new(),
                },
                round_card(
                    1,
                    Dot::Working,
                    (
                        "2 findings \u{b7} making fixes \u{b7} 1 of 2 made",
                        CheckTone::Note,
                    ),
                    "asked by AI review 30m ago \u{b7} made by the Build agent (Claude Code) \u{b7} \
                     fixes went 20m ago",
                    &[
                        (true, "r1-1: Export drops the last row"),
                        (false, "r1-2: Long jobs lose their progress"),
                    ],
                    true,
                ),
                PageSection::Milestones {
                    label: "Build".into(),
                    state: None,
                    done: 3,
                    total: 5,
                    items: [
                        (true, "Export query + streaming CSV"),
                        (true, "Background job + progress"),
                        (true, "Download endpoint"),
                        (false, "Web: export dialog"),
                        (false, "Email when ready"),
                    ]
                    .map(|(d, t)| (d, t.to_owned()))
                    .into(),
                    running: vec![3, 4],
                    buttons: Vec::new(),
                },
            ],
        },
        activity: activity.into(),
        running: Vec::new(),
        running_more: 0,
        scroll: [0.0; 3],
        hover: None,
        hints: hints.map(|(k, a)| (k.to_owned(), a.to_owned())).into(),
        marks: Default::default(),
    }
}

/// `view` with stage `i` the selected one.
fn select_node(view: &mut super::FeatureTabView, i: usize) {
    for (j, node) in view.rail.iter_mut().enumerate() {
        node.selected = i == j;
    }
}

/// `view` at stage `current`: the ones before it done, it marked `mark`.
fn at_stage(view: &mut super::FeatureTabView, current: usize, mark: super::NodeMark, line: &str) {
    use super::NodeMark;
    for (j, node) in view.rail.iter_mut().enumerate() {
        node.mark = match j {
            j if j < current => NodeMark::Done,
            j if j == current => mark,
            _ => NodeMark::Later,
        };
        node.line = match j {
            j if j < current => "1h ago".into(),
            j if j == current => line.into(),
            _ => String::new(),
        };
    }
    select_node(view, current);
}

/// Plan written and waiting for review: the planner idle.
fn feature_tab_plan_ready() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, Dot, NodeMark, PageButton, PageSection, StagePage};
    use crate::{agent_icons::AgentIcon, feature::Stage};
    let mut view = feature_tab();
    at_stage(&mut view, 1, NodeMark::Waiting, "ready for review");
    let button = |label: &str, style| PageButton {
        label: label.into(),
        style,
    };
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Plan".into(),
        about: Stage::Plan.about().into(),
        status: Some(tab_line("plan.md is ready for review", CheckTone::Blocker)),
        buttons: vec![
            button("Review", ButtonStyle::Primary),
            button("Show agent", ButtonStyle::Plain),
            button("Approve", ButtonStyle::Plain),
        ],
        sections: vec![
            PageSection::Agent {
                stage: crate::feature::Stage::Plan,
                icon: AgentIcon::Codex,
                name: "codex".into(),
                dot: Dot::Idle,
                status: "your turn \u{b7} tab 2".into(),
                tail: Vec::new(),
                buttons: vec![1],
                picks: Vec::new(),
                model: None,
                waits: false,
            },
            PageSection::File {
                name: "plan.md".into(),
                state: tab_line("ready for review", CheckTone::Blocker),
                buttons: vec![0, 2],
            },
        ],
    };
    view
}

/// A brief as a file's page shows it, every kind of block in it.
fn doc_blocks() -> Vec<crate::md_blocks::DocBlock> {
    use crate::md_blocks::{DocBlock, ItemMark, Styled, blocks};
    // Markdown's words with their runs: bold, code and links.
    let styled = |md: &str| match blocks(md).remove(0) {
        DocBlock::Para(text) => text,
        _ => Styled::from(md),
    };
    let item = |depth, mark, text: &str| DocBlock::Item {
        depth,
        mark,
        text: styled(text),
    };
    vec![
        DocBlock::Heading {
            level: 1,
            text: "Log in with a password".into(),
        },
        DocBlock::Para(styled(
            "People sign in with an email and a **password**, and stay signed in. A wrong \
             password says so, without saying which of the two it was: `POST /login` \
             answers 401, as [the spec](https://example.com/spec) has it.",
        )),
        DocBlock::Heading {
            level: 2,
            text: "What it does".into(),
        },
        item(
            0,
            ItemMark::Bullet,
            "After five tries the account waits a minute, and says for how long.",
        ),
        item(
            1,
            ItemMark::Bullet,
            "The wait is per account, not per address.",
        ),
        item(
            0,
            ItemMark::Number(2),
            "A reset link goes to the email, and works once.",
        ),
        item(0, ItemMark::Task(true), "Sessions last thirty days."),
        item(
            0,
            ItemMark::Task(false),
            "See https://example.com/a-link-too-long-to-fit-on-any-line-of-this-column-so-it-breaks-where-it-must-to-keep-all-of-it",
        ),
        DocBlock::Quote("Nobody asked for a second factor yet, and this doesn't add one.".into()),
        DocBlock::Code(vec![
            "POST /login".into(),
            "{ \"email\": \"a@b.c\", \"password\": \"...\" }".into(),
        ]),
        DocBlock::Table(vec![
            "Case \u{b7} Reply".into(),
            "Wrong password \u{b7} 401".into(),
            "Too many tries \u{b7} 429".into(),
        ]),
        DocBlock::Rule,
        DocBlock::Note("Too big to show here: use Review".into()),
    ]
}

/// Discover's brief ready: the page reads it, a button reviews it.
fn feature_tab_discover_brief() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, Dot, NodeMark, PageButton, PageSection, StagePage};
    use crate::{agent_icons::AgentIcon, feature::Stage};
    let mut view = feature_tab();
    at_stage(&mut view, 0, NodeMark::Waiting, "ready for review");
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Discover".into(),
        about: Stage::Discover.about().into(),
        status: Some(tab_line("brief.md is ready for review", CheckTone::Blocker)),
        buttons: vec![
            PageButton {
                label: "Review".into(),
                style: ButtonStyle::Primary,
            },
            PageButton {
                label: "Show agent".into(),
                style: ButtonStyle::Plain,
            },
            PageButton {
                label: "Approve".into(),
                style: ButtonStyle::Plain,
            },
        ],
        sections: vec![
            PageSection::Agent {
                stage: crate::feature::Stage::Discover,
                icon: AgentIcon::Claude,
                name: "claude".into(),
                dot: Dot::Idle,
                status: "your turn \u{b7} tab 2".into(),
                tail: Vec::new(),
                buttons: vec![1],
                picks: Vec::new(),
                model: None,
                waits: false,
            },
            PageSection::File {
                name: "brief.md".into(),
                state: tab_line("ready for review", CheckTone::Blocker),
                buttons: vec![0, 2],
            },
            PageSection::Doc {
                name: "brief.md".into(),
                blocks: std::sync::Arc::new(doc_blocks()),
                here: false,
            },
        ],
    };
    view
}

/// Discover's brief ready with its decision log: a card of its own, with
/// Open.
fn feature_tab_discover_decisions() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, PageButton, PageSection};
    let mut view = feature_tab_discover_brief();
    view.page.buttons.insert(
        2,
        PageButton {
            label: "Open".into(),
            style: ButtonStyle::Plain,
        },
    );
    let sections = &mut view.page.sections;
    if let Some(PageSection::File { buttons, .. }) = sections.get_mut(1) {
        *buttons = vec![0, 3];
    }
    sections.insert(
        2,
        PageSection::File {
            name: "decisions.md".into(),
            state: tab_line("4 decisions", CheckTone::Note),
            buttons: vec![2],
        },
    );
    view
}

/// Discover's page with its research: one running, one answered, one
/// stopped, one waiting its turn, each a card under the agent's.
fn feature_tab_discover_research() -> super::FeatureTabView {
    use super::{ButtonStyle, Dot, PageButton, PageSection};
    use crate::{agent_icons::AgentIcon, feature::Stage};
    let mut view = feature_tab_discover_decisions();
    let first = view.page.buttons.len();
    for label in ["Show agent", "Open", "Run again"] {
        view.page.buttons.push(PageButton {
            label: label.into(),
            style: ButtonStyle::Plain,
        });
    }
    let card = |id: u32, icon, dot, status: &str, buttons: Vec<usize>, waits| PageSection::Agent {
        stage: Stage::Discover,
        icon,
        name: format!("Research R{id}"),
        dot,
        status: status.into(),
        tail: Vec::new(),
        buttons,
        picks: Vec::new(),
        model: None,
        waits,
    };
    let cards = [
        card(
            1,
            AgentIcon::Claude,
            Dot::Working,
            "Does the Stripe invoices API page its results? \u{b7} working \u{b7} started 2m ago",
            vec![first],
            false,
        ),
        card(
            2,
            AgentIcon::Claude,
            Dot::Done,
            "Which CSV dialect does the accountant's tool read? \u{b7} answered 1m ago",
            vec![first + 1],
            false,
        ),
        card(
            3,
            AgentIcon::Claude,
            Dot::Failed,
            "Can exports run in the background? \u{b7} stopped \u{b7} its tab closed",
            vec![first + 2],
            false,
        ),
        card(
            4,
            AgentIcon::from_agent(""),
            Dot::Unknown,
            "Is there a rate limit on exports? \u{b7} waits its turn",
            Vec::new(),
            false,
        ),
    ];
    view.page.sections.splice(1..1, cards);
    view
}

/// A plan too long for its column, scrolled.
fn feature_tab_plan_long() -> super::FeatureTabView {
    use super::{CheckTone, NodeMark, PageSection, TabColumn};
    let mut view = feature_tab_plan_ready();
    at_stage(&mut view, 1, NodeMark::Waiting, "ready for review");
    let blocks = (0..12).flat_map(|_| doc_blocks()).collect();
    view.page.sections.push(PageSection::Doc {
        name: "plan.md".into(),
        blocks: std::sync::Arc::new(blocks),
        here: false,
    });
    view.page.status = Some(tab_line("plan.md is ready for review", CheckTone::Blocker));
    view.scroll[TabColumn::Page as usize] = 300.0;
    view
}

/// Plan done, its agent's tab closed: its chat a button away.
fn feature_tab_resume() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, Dot, NodeMark, PageButton, PageSection, StagePage};
    use crate::{agent_icons::AgentIcon, feature::Stage};
    let mut view = feature_tab();
    select_node(&mut view, 1);
    view.rail[1].mark = NodeMark::Done;
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Plan".into(),
        about: Stage::Plan.about().into(),
        status: Some(tab_line("Done 2h ago by you", CheckTone::Ready)),
        buttons: vec![
            PageButton {
                label: "Reopen".into(),
                style: ButtonStyle::Plain,
            },
            PageButton {
                label: "Resume agent".into(),
                style: ButtonStyle::Plain,
            },
        ],
        sections: vec![PageSection::Agent {
            stage: crate::feature::Stage::Plan,
            icon: AgentIcon::Claude,
            name: "claude".into(),
            dot: Dot::Done,
            status: "started 2h ago".into(),
            tail: Vec::new(),
            buttons: vec![1],
            picks: Vec::new(),
            model: None,
            waits: false,
        }],
    };
    view
}

/// Verify failed: two checks fail, fixes to plan.
fn feature_tab_verify_failed() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, NodeMark, PageButton, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 4, NodeMark::Current, "failed");
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Verify".into(),
        about: Stage::Verify.about().into(),
        status: Some(tab_line("2 of 5 checks failed", CheckTone::Blocker)),
        buttons: vec![
            PageButton {
                label: "Plan fixes".into(),
                style: ButtonStyle::Primary,
            },
            PageButton {
                label: "Run again".into(),
                style: ButtonStyle::Plain,
            },
            PageButton {
                label: "Evidence".into(),
                style: ButtonStyle::Plain,
            },
        ],
        sections: vec![PageSection::Lines {
            label: "Checks".into(),
            lines: vec![
                tab_line("cargo build passed", CheckTone::Ready),
                tab_line("cargo clippy passed", CheckTone::Ready),
                tab_line(
                    "cargo test failed: export::csv::streams_large_files panicked at \
                     'assertion failed: rows == 10000', api/src/export/csv.rs:212",
                    CheckTone::Blocker,
                ),
                tab_line(
                    "pnpm test failed: Dialog.test.tsx (1 of 14)",
                    CheckTone::Blocker,
                ),
                tab_line("pnpm lint passed", CheckTone::Ready),
            ],
        }],
    };
    view
}

/// Verify failed by a check that passed the run before: what changed since
/// broke it. Its fixes are planned first all the same.
fn feature_tab_verify_regressed() -> super::FeatureTabView {
    use super::{CheckTone, PageSection};
    let mut view = feature_tab_verify_failed();
    view.page.status = Some(tab_line(
        "failed: api: `cargo test` exited with 101; it passed before the last change",
        CheckTone::Blocker,
    ));
    view.page.buttons = verify_failed_buttons(1);
    view.page.sections = vec![PageSection::Lines {
        label: "Checks".into(),
        lines: vec![
            tab_line("cargo build passed", CheckTone::Ready),
            tab_line(
                "cargo test failed: export::csv::streams_large_files: no method named \
                 `stream` found for struct `Export`, api/src/export/csv.rs:212",
                CheckTone::Blocker,
            ),
            tab_line("pnpm lint passed", CheckTone::Ready),
        ],
    }];
    view
}

/// Verify failed by the machine, not the code: a full disk. No fixes are
/// planned, Run failed is what comes next.
fn feature_tab_verify_machine() -> super::FeatureTabView {
    use super::{CheckTone, PageSection};
    let mut view = feature_tab_verify_failed();
    view.page.status = Some(tab_line(
        "failed because of the machine, not its code: Disk quota exceeded; run the \
         failed checks again",
        CheckTone::Blocker,
    ));
    view.page.buttons = verify_failed_buttons(2);
    view.page.sections = vec![PageSection::Lines {
        label: "Checks".into(),
        lines: vec![
            tab_line("cargo build passed", CheckTone::Ready),
            tab_line(
                "pytest failed: OSError: [Errno 122] Disk quota exceeded",
                CheckTone::Blocker,
            ),
            tab_line("pnpm lint passed", CheckTone::Ready),
        ],
    }];
    view
}

/// The buttons of Verify's page after a failure, `primary` the one Enter
/// presses, in the order the page lists them.
fn verify_failed_buttons(primary: usize) -> Vec<super::PageButton> {
    use super::{ButtonStyle, PageButton};
    let labels = [
        "Make fixes",
        "Plan fixes",
        "Run failed",
        "Run checks",
        "Evidence",
    ];
    (labels.into_iter().enumerate())
        .map(|(i, label)| PageButton {
            label: label.into(),
            style: match i == primary {
                true => ButtonStyle::Primary,
                false => ButtonStyle::Plain,
            },
        })
        .collect()
}

/// Verify with its checks running: Stop all at the title's right, and only
/// what works during a run beside it.
fn feature_tab_verify_running() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, NodeMark, PageButton, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 4, NodeMark::Current, "running");
    let button = |label: &str, style| PageButton {
        label: label.into(),
        style,
    };
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Verify".into(),
        about: Stage::Verify.about().into(),
        status: Some(tab_line(
            "Verifying \u{201c}Export\u{201d}",
            CheckTone::Note,
        )),
        buttons: vec![
            button("Stop all", ButtonStyle::Plain),
            button("Evidence", ButtonStyle::Plain),
            button("Setup log", ButtonStyle::Quiet),
            button("Commands\u{2026}", ButtonStyle::Quiet),
        ],
        sections: vec![PageSection::Lines {
            label: "Checks \u{b7} 1 passed \u{b7} 2 running".into(),
            lines: Vec::new(),
        }],
    };
    // The checks and the code graph run, shown under the rail.
    let row = |word, what: &str, steps, took: &str, buttons, node| super::RunningRow {
        word,
        what: what.into(),
        steps,
        took: took.into(),
        buttons,
        node,
    };
    view.running = vec![
        row("checks", "", Some((1, 3)), "0:41", vec!["Stop"], Some(4)),
        row("code graph", "", None, "0:07", Vec::new(), None),
    ];
    view
}

/// `view` with the flow back at Build from stage `from`: what was done from
/// Build on ran once.
fn turned_back(view: &mut super::FeatureTabView, from: usize) {
    use super::NodeMark;
    for node in &mut view.rail[3..] {
        if node.mark == NodeMark::Done {
            node.mark = NodeMark::Ran;
        }
    }
    view.back = Some(super::RailBack { from, to: 3 });
}

/// A fix plan from Verify's failures, ready to review.
fn feature_tab_fix_plan() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, NodeMark, PageButton, PageSection};
    let mut view = feature_tab_verify_failed();
    at_stage(&mut view, 4, NodeMark::Waiting, "fix plan ready");
    turned_back(&mut view, 4);
    view.page.buttons = vec![
        PageButton {
            label: "Review".into(),
            style: ButtonStyle::Primary,
        },
        PageButton {
            label: "Make fixes".into(),
            style: ButtonStyle::Plain,
        },
        PageButton {
            label: "Drop plan\u{2026}".into(),
            style: ButtonStyle::Quiet,
        },
        PageButton {
            label: "Run again".into(),
            style: ButtonStyle::Plain,
        },
    ];
    view.page.sections.insert(
        0,
        PageSection::File {
            name: "Fix plan".into(),
            state: tab_line("Verify failures \u{b7} fix plan ready", CheckTone::Blocker),
            buttons: vec![0, 1, 2],
        },
    );
    view
}

/// Build's page after Verify's failures came back to it twice, and a setup.
fn feature_tab_verify_fixes() -> super::FeatureTabView {
    use super::{CheckTone, Dot, PageSection, RoundAsked};
    let mut view = feature_tab();
    for section in &mut view.page.sections {
        match section {
            PageSection::Lines { label, .. } if label == "Fix round" => {
                *label = "Fix rounds \u{b7} 2".into();
            }
            PageSection::Round(card) if card.label == fix_label(1) => {
                card.dot = Dot::Done;
                card.state = tab_line("2 of 2 made", CheckTone::Ready);
                card.meta =
                    "asked by Verify 2h ago \u{b7} made by the Build agent (Cursor) \u{b7} \
                             run 2 failed 2h ago \u{b7} finished 1h ago \u{b7} run 3 failed"
                        .into();
                card.open = false;
            }
            _ => {}
        }
    }
    let PageSection::Round(newest) = round_card(
        2,
        Dot::Working,
        (
            "Verify's failures \u{b7} making fixes \u{b7} 1 of 3 made",
            CheckTone::Note,
        ),
        "asked by Verify 12m ago \u{b7} made by the Build agent (Claude Code) \u{b7} \
         run 3 failed 12m ago \u{b7} fixes went 8m ago",
        &[
            (true, "Export streams its rows"),
            (false, "api: `cargo test export::` passes"),
            (false, "web: `npm run lint` passes"),
        ],
        true,
    ) else {
        unreachable!()
    };
    let asked = |title: &str, note: &str| RoundAsked {
        id: String::new(),
        title: title.into(),
        note: note.into(),
        now: Some(tab_line("being fixed", CheckTone::Note)),
    };
    let newest = super::RoundCard {
        asked: vec![
            asked(
                "api: `cargo test export::` exited with 101",
                "error[E0599]: no method named `stream` found for struct `Export` in the current scope, which is long enough to reach the side of the page at the narrow width",
            ),
            asked("web: `npm run lint` exited with 2", ""),
        ],
        ..newest
    };
    view.page.sections.insert(2, PageSection::Round(newest));
    view.page.sections.push(PageSection::Lines {
        label: "Setup".into(),
        lines: vec![
            tab_line("copied api: .env", CheckTone::Note),
            tab_line("ran web: npm ci", CheckTone::Note),
        ],
    });
    view
}

/// Build's page with Code review's round open: its plan waits, so nothing is
/// in the build log yet, and another round is still being planned above it.
fn feature_tab_open_round() -> super::FeatureTabView {
    use super::{CheckTone, Dot, PageSection};
    let mut view = feature_tab();
    for section in &mut view.page.sections {
        match section {
            PageSection::Lines { label, .. } if label == "Fix round" => {
                *label = "Fix rounds \u{b7} 3".into();
            }
            PageSection::Round(card) if card.label == fix_label(1) => {
                card.dot = Dot::Done;
                card.state = tab_line("2 of 2 made", CheckTone::Ready);
                card.open = false;
            }
            _ => {}
        }
    }
    view.page.sections.insert(
        2,
        round_card(
            3,
            Dot::Working,
            (
                "manual test failures \u{b7} planning fixes",
                CheckTone::Note,
            ),
            "asked by Manual test 2m ago",
            &[],
            true,
        ),
    );
    view.page.sections.insert(
        3,
        round_card(
            2,
            Dot::Idle,
            ("review comments \u{b7} fix plan ready", CheckTone::Blocker),
            "asked by Code review 20m ago \u{b7} plan written 5m ago",
            &[
                (false, "api/src/export/csv.rs:32-38"),
                (false, "All the fixes of the plan"),
            ],
            false,
        ),
    );
    view
}

/// Ship current, blocked by what isn't committed and a branch behind.
fn feature_tab_ship_blocked() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, NodeMark, PageButton, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 8, NodeMark::Current, "blocked");
    view.repos[1].line = "feature/bulk-export \u{b7} push not guarded: hooks path is set".into();
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Ship".into(),
        about: Stage::Ship.about().into(),
        status: None,
        buttons: vec![
            PageButton {
                label: "Ship\u{2026}".into(),
                style: ButtonStyle::Off,
            },
            PageButton {
                label: "Check again".into(),
                style: ButtonStyle::Quiet,
            },
        ],
        sections: vec![
            PageSection::Lines {
                label: "Ship check".into(),
                lines: vec![
                    tab_line("api has changes not committed", CheckTone::Blocker),
                    tab_line("web is 3 commits behind dev", CheckTone::Blocker),
                ],
            },
            PageSection::Toolbar {
                groups: vec![(String::new(), vec![1])],
            },
            PageSection::ShipGroup(ship_blocked(ship_group(
                "api \u{b7} 4 commits \u{b7} +120 \u{2212}8 \u{b7} into main",
                1,
                0,
                None,
                vec![
                    ship_row(1, "Push feature/bulk-export to origin", None, false),
                    ship_row(
                        2,
                        "Merge into main in ~/code/api",
                        Some(("api's main worktree is on dev: switch it to main", true)),
                        true,
                    ),
                    ship_row(3, "Push main to origin", None, false),
                ],
            ))),
            PageSection::ShipGroup(ship_afterwards(4, false)),
        ],
    };
    view
}

/// The Ship page while its first check runs: a card for each repository,
/// its dot blinking, until what the check found takes its place.
fn feature_tab_ship_checking() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, Dot, NodeMark, PageButton, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 8, NodeMark::Current, "checking");
    let card = |head: &str| {
        PageSection::ShipGroup(super::ShipGroup {
            head: head.into(),
            caption: false,
            card: Some(super::ShipCard {
                dot: Dot::Working,
                state: tab_line("checking\u{2026}", CheckTone::Note),
                meta: "reading its git status and fetching origin".into(),
                base: None,
                ci: None,
                buttons: Vec::new(),
            }),
            route: None,
            note: None,
            steps: Vec::new(),
            selected: None,
        })
    };
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Ship".into(),
        about: Stage::Ship.about().into(),
        status: None,
        buttons: vec![
            PageButton {
                label: "Ship\u{2026}".into(),
                style: ButtonStyle::Off,
            },
            PageButton {
                label: "Check again".into(),
                style: ButtonStyle::Quiet,
            },
        ],
        sections: vec![
            PageSection::Lines {
                label: "Ship check".into(),
                lines: vec![tab_line(
                    "Checking the worktrees and fetching origin\u{2026}",
                    CheckTone::Note,
                )],
            },
            PageSection::Toolbar {
                groups: vec![(String::new(), vec![1])],
            },
            card("api"),
            card("web"),
        ],
    };
    view
}

/// A repository's card on the Ship page, `head` its name then what it
/// holds: `row` its choice's row, `on` its way, if any.
fn ship_group(
    head: &str,
    on: usize,
    row: usize,
    note: Option<&str>,
    steps: Vec<super::ShipStepRow>,
) -> super::ShipGroup {
    use super::{CheckTone, Dot};
    let (name, meta) = head.split_once(" \u{b7} ").unwrap_or((head, ""));
    let left = steps.iter().filter(|s| s.ticked && !s.done).count();
    let state = match left {
        0 => tab_line("stays as it is", CheckTone::Note),
        1 => tab_line("1 step to run", CheckTone::Note),
        n => tab_line(&format!("{n} steps to run"), CheckTone::Note),
    };
    super::ShipGroup {
        head: name.into(),
        caption: false,
        card: Some(super::ShipCard {
            dot: Dot::Idle,
            state,
            meta: meta.into(),
            base: None,
            ci: None,
            buttons: Vec::new(),
        }),
        route: Some(super::ShipRouteRow { row, on }),
        note: note.map(str::to_owned),
        steps,
        selected: None,
    }
}

/// `group` held up by a step that can't run.
fn ship_blocked(mut group: super::ShipGroup) -> super::ShipGroup {
    if let Some(card) = group.card.as_mut() {
        card.dot = super::Dot::Blocked;
        card.state = tab_line("can't run yet", super::CheckTone::Blocker);
    }
    group
}

/// A step of a Ship card that ran, `when` it did.
fn ship_ran(label: &str, when: &str) -> super::ShipStepRow {
    super::ShipStepRow {
        row: 0,
        label: label.into(),
        detail: Some(tab_line(when, super::CheckTone::Note)),
        ticked: true,
        off: false,
        done: true,
        button: None,
    }
}

/// A tick box of the Ship page: `detail` says what goes under it and whether
/// that is why it can't run.
fn ship_row(
    row: usize,
    label: &str,
    detail: Option<(&str, bool)>,
    ticked: bool,
) -> super::ShipStepRow {
    use super::CheckTone;
    let tone = |blocks| match blocks {
        true => CheckTone::Blocker,
        false => CheckTone::Note,
    };
    super::ShipStepRow {
        row,
        label: label.into(),
        detail: detail.map(|(text, blocks)| tab_line(text, tone(blocks))),
        ticked,
        off: detail.is_some_and(|(_, blocks)| blocks),
        done: false,
        button: None,
    }
}

/// What follows shipping, from row `row` on; `removable`: all of it merges.
fn ship_afterwards(row: usize, removable: bool) -> super::ShipGroup {
    let mut remove = ship_row(
        row + 1,
        "Remove the worktree and branch",
        (!removable).then_some(("Only when every repository merges into its base", false)),
        false,
    );
    remove.off = !removable;
    super::ShipGroup {
        head: "Afterwards".into(),
        caption: true,
        card: None,
        route: None,
        note: None,
        steps: vec![ship_row(row, "Close the workspace", None, true), remove],
        selected: None,
    }
}

/// Ship with three repositories going three ways, the keyboard on a tick box
/// and the pointer on a choice.
fn feature_tab_ship_routes() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, NodeMark, PageButton, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 8, NodeMark::Current, "ready to ship");
    let mut api = ship_group(
        "api \u{b7} 12 commits \u{b7} +420 \u{2212}80 \u{b7} into main",
        0,
        0,
        None,
        vec![
            ship_row(1, "Push feature/bulk-export to origin", None, true),
            ship_row(2, "Open a pull request into main", None, false),
        ],
    );
    api.selected = Some(3);
    let mut web = ship_group(
        "web \u{b7} 3 commits \u{b7} +40 \u{2212}2 \u{b7} into dev",
        1,
        3,
        None,
        vec![
            ship_row(4, "Push feature/bulk-export to origin", None, true),
            ship_row(
                5,
                "Merge into dev in ~/code/web",
                Some(("Squash; you edit its message when you ship", false)),
                true,
            ),
            ship_row(6, "Push dev to origin", None, true),
        ],
    );
    web.selected = Some(3);
    let mut docs = ship_group(
        "docs \u{b7} 1 commit \u{b7} +4 \u{2212}0 \u{b7} into main",
        2,
        7,
        Some("Nothing gets pushed or merged"),
        Vec::new(),
    );
    docs.selected = Some(3);
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Ship".into(),
        about: Stage::Ship.about().into(),
        status: None,
        buttons: vec![
            PageButton {
                label: "Ship\u{2026}".into(),
                style: ButtonStyle::Primary,
            },
            PageButton {
                label: "Start over\u{2026}".into(),
                style: ButtonStyle::Quiet,
            },
        ],
        sections: vec![
            PageSection::Lines {
                label: "Ship check".into(),
                lines: vec![
                    tab_line("Ready to ship", CheckTone::Ready),
                    tab_line("lib has no commits: left out", CheckTone::Note),
                ],
            },
            PageSection::ShipGroup(api),
            PageSection::ShipGroup(web),
            PageSection::ShipGroup(docs),
            PageSection::ShipGroup(ship_afterwards(8, false)),
        ],
    };
    view.hover = Some(super::FeatureTabHit::ShipRoute(3, 2));
    view
}

/// Ship with no step ticked: it finishes, the pointer on a tick box.
fn feature_tab_ship_finish() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, NodeMark, PageButton, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 8, NodeMark::Current, "nothing ticked");
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Ship".into(),
        about: Stage::Ship.about().into(),
        status: None,
        buttons: vec![PageButton {
            label: "Finish\u{2026}".into(),
            style: ButtonStyle::Primary,
        }],
        sections: vec![
            PageSection::Lines {
                label: "Ship check".into(),
                lines: vec![tab_line(
                    "No step ticked: Finish ends it without shipping",
                    CheckTone::Note,
                )],
            },
            PageSection::ShipGroup(ship_group(
                "api \u{b7} 12 commits \u{b7} +420 \u{2212}80 \u{b7} into main",
                0,
                0,
                None,
                vec![
                    ship_row(1, "Push feature/bulk-export to origin", None, false),
                    ship_row(2, "Open a pull request into main", None, false),
                ],
            )),
            PageSection::ShipGroup(ship_afterwards(3, false)),
        ],
    };
    view.hover = Some(super::FeatureTabHit::ShipRow(1));
    view
}

/// The recap of a feature made in two days, its summary written.
fn recap() -> super::RecapCard {
    let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    super::RecapCard {
        summary: vec![
            "Invoices can now be exported in bulk: pick a date range and a format, and \
             the export runs in the background, with a link mailed once it is ready."
                .into(),
            "The export job reuses the PDF renderer; CSV goes through a new streaming \
             writer so large ranges stay within memory."
                .into(),
        ],
        summary_note: Some("Goes in the pull requests and the squash's message".into()),
        figures: vec![
            pair("2d 4h", "start to ship"),
            pair("15", "commits"),
            pair("+460 \u{2212}82", "lines"),
            pair("31", "files"),
            pair("2", "repositories"),
            pair("14", "agent runs"),
            pair("3", "fix rounds"),
        ],
        stages: vec![
            pair(
                "Discover",
                "Claude Code opus-4 \u{b7} 2 drafts \u{b7} took 40m",
            ),
            pair("Plan", "Claude Code opus-4 \u{b7} took 1h 5m"),
            pair(
                "Plan review",
                "Codex gpt-5 \u{b7} 6 suggestions \u{b7} 4 taken \u{b7} took 20m",
            ),
            pair("Build", "Cursor \u{b7} 3 runs \u{b7} took 9h 30m"),
            pair(
                "Verify",
                "12 of 12 checks passed \u{b7} 4 runs \u{b7} took 2h",
            ),
            pair(
                "AI review",
                "Codex gpt-5 \u{b7} 9 findings \u{b7} 7 fixed \u{b7} 2 dismissed \u{b7} took 3h",
            ),
            pair("Code review", "1 round of comments \u{b7} took 1d 2h"),
            pair("Manual test", "8 of 8 criteria passed \u{b7} took 4h"),
        ],
        lines: Vec::new(),
        buttons: vec![3, 4],
    }
}

/// A shipped feature that can ship more: api's pull request open, web
/// merged with its base yet to push, and what ran in the Ship log.
fn feature_tab_ship_live() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, Dot, NodeMark, PageButton, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 8, NodeMark::Done, "2h ago");
    let mut api = ship_group(
        "api \u{b7} 12 commits \u{b7} +420 \u{2212}80 \u{b7} into main",
        0,
        0,
        None,
        vec![
            ship_ran(
                "Pushed feature/bulk-export to origin",
                "2h ago \u{b7} 3f9c2a1",
            ),
            ship_ran(
                "Opened a pull request into main",
                "2h ago \u{b7} api/pull/10",
            ),
        ],
    );
    api.steps[0].button = Some(5);
    api.steps[1].button = Some(6);
    let card = api.card.as_mut().unwrap();
    card.dot = Dot::Done;
    card.state = tab_line("pull request open", CheckTone::Ready);
    card.buttons = vec![1];
    let mut web = ship_group(
        "web \u{b7} 3 commits \u{b7} +40 \u{2212}2 \u{b7} into dev",
        1,
        0,
        None,
        vec![
            ship_ran("Merged into dev", "2h ago"),
            ship_row(1, "Push dev to origin", None, true),
        ],
    );
    web.route = None;
    web.steps[0].button = Some(7);
    web.card.as_mut().unwrap().state = tab_line("merged into dev: push it", CheckTone::Note);
    web.selected = Some(1);
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Ship".into(),
        about: Stage::Ship.about().into(),
        status: Some(tab_line("Done 2h ago by gerdr", CheckTone::Ready)),
        buttons: vec![
            PageButton {
                label: "Ship 1 step\u{2026}".into(),
                style: ButtonStyle::Primary,
            },
            PageButton {
                label: "Open PR".into(),
                style: ButtonStyle::Plain,
            },
            PageButton {
                label: "Reopen".into(),
                style: ButtonStyle::Plain,
            },
            PageButton {
                label: "Edit".into(),
                style: ButtonStyle::Quiet,
            },
            PageButton {
                label: "Write again".into(),
                style: ButtonStyle::Quiet,
            },
            PageButton {
                label: "Undo\u{2026}".into(),
                style: ButtonStyle::Quiet,
            },
            PageButton {
                label: "Undo\u{2026}".into(),
                style: ButtonStyle::Quiet,
            },
            PageButton {
                label: "Undo\u{2026}".into(),
                style: ButtonStyle::Quiet,
            },
        ],
        sections: vec![
            PageSection::Lines {
                label: "Ship check".into(),
                lines: vec![tab_line("Shipped: 1 step ticked to run", CheckTone::Ready)],
            },
            PageSection::Recap(recap()),
            PageSection::ShipGroup(api),
            PageSection::ShipGroup(web),
            PageSection::Lines {
                label: "Ship log".into(),
                lines: vec![
                    tab_line("2h ago \u{b7} web: squashed into dev", CheckTone::Note),
                    tab_line(
                        "2h ago \u{b7} api: opened a pull request \u{b7} api/pull/10",
                        CheckTone::Note,
                    ),
                    tab_line(
                        "2h ago \u{b7} api: pushed feature/bulk-export \u{b7} 3f9c2a1",
                        CheckTone::Note,
                    ),
                ],
            },
        ],
    };
    view
}

/// `feature_tab_ship_live` with its recap folded away: its cards, their
/// Undo buttons, and the Ship log in view.
fn feature_tab_ship_cards() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, PageButton, PageSection};
    let mut view = feature_tab_ship_live();
    (view.page.sections).retain(|s| !matches!(s, PageSection::Recap(_)));
    // Its base moved on since the pull request opened.
    let at = view.page.buttons.len();
    for (label, style) in [
        ("Merge main in\u{2026}", ButtonStyle::Quiet),
        ("Rebase\u{2026}", ButtonStyle::Quiet),
        ("Merge main into all\u{2026}", ButtonStyle::Plain),
    ] {
        view.page.buttons.push(PageButton {
            label: label.into(),
            style,
        });
    }
    if let Some(PageSection::ShipGroup(api)) = view.page.sections.get_mut(1)
        && let Some(card) = api.card.as_mut()
    {
        card.base = Some(tab_line(
            "origin/main has 3 new commits \u{b7} merges cleanly",
            CheckTone::Note,
        ));
        card.buttons = vec![at, at + 1, 1];
    }
    // Every repository's at once, under the Ship check.
    view.page.sections.insert(
        1,
        PageSection::Toolbar {
            groups: vec![(String::new(), vec![at + 2])],
        },
    );
    view
}

/// `feature_tab_ship_cards` once its workspace closed after shipping and
/// its worktrees were removed and restored: what followed, with the Undo
/// bringing the workspace's tabs back, and in the Ship log.
fn feature_tab_ship_after() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, PageButton, PageSection, ShipGroup};
    let mut view = feature_tab_ship_cards();
    let at = view.page.buttons.len();
    view.page.buttons.push(PageButton {
        label: "Undo".into(),
        style: ButtonStyle::Quiet,
    });
    let mut closed = ship_ran("Closed the workspace", "2h ago");
    closed.button = Some(at);
    let removed = ship_ran(
        "Removed the worktrees and branches",
        "2h ago \u{b7} restored 5m ago",
    );
    let group = ShipGroup {
        head: "Afterwards".into(),
        caption: true,
        card: None,
        route: None,
        note: None,
        steps: vec![closed, removed],
        selected: None,
    };
    let log = (view.page.sections.iter())
        .position(|s| matches!(s, PageSection::Lines { label, .. } if label == "Ship log"));
    let log = log.unwrap_or(view.page.sections.len());
    view.page
        .sections
        .insert(log, PageSection::ShipGroup(group));
    if let Some(PageSection::Lines { lines, .. }) = view.page.sections.get_mut(log + 1) {
        lines.insert(
            0,
            tab_line(
                "2h ago \u{b7} removed the worktrees \u{b7} undone 5m ago",
                CheckTone::Note,
            ),
        );
        lines.insert(
            1,
            tab_line("2h ago \u{b7} closed the workspace", CheckTone::Note),
        );
    }
    view
}

/// `feature_tab_ship_cards` with api's base conflicting: the agent's two
/// ways to take that on, for api on its card and for all under the Ship
/// check.
fn feature_tab_ship_conflicts() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, PageButton, PageSection};
    let mut view = feature_tab_ship_cards();
    let at = view.page.buttons.len() - 3;
    view.page.buttons[at].label = "Fix".into();
    view.page.buttons[at + 1].label = "Plan".into();
    view.page.buttons[at + 2].label = "Resolve all with agent".into();
    view.page.buttons.push(PageButton {
        label: "Plan resolution of all".into(),
        style: ButtonStyle::Plain,
    });
    if let Some(PageSection::Toolbar { groups }) = view.page.sections.get_mut(1) {
        groups[0].1.push(at + 3);
    }
    if let Some(PageSection::ShipGroup(api)) = view.page.sections.get_mut(2)
        && let Some(card) = api.card.as_mut()
    {
        card.base = Some(tab_line(
            "origin/main has 4 new commits \u{b7} conflicts in src/export.rs, src/routes.rs",
            CheckTone::Blocker,
        ));
    }
    view
}

/// `feature_tab_ship_live` with checks failing on api's pull request: what
/// failed, and the ways to fix them or run them again.
fn feature_tab_ship_ci() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, Dot, PageButton, PageSection};
    let mut view = feature_tab_ship_live();
    (view.page.sections).retain(|s| !matches!(s, PageSection::Recap(_)));
    let at = view.page.buttons.len();
    for (label, style) in [
        ("Fix CI", ButtonStyle::Primary),
        ("Plan CI fixes", ButtonStyle::Quiet),
        ("Rerun failed", ButtonStyle::Plain),
    ] {
        view.page.buttons.push(PageButton {
            label: label.into(),
            style,
        });
    }
    if let Some(PageSection::ShipGroup(api)) = view.page.sections.get_mut(1)
        && let Some(card) = api.card.as_mut()
    {
        card.dot = Dot::Failed;
        card.state = tab_line(
            "pull request open \u{b7} 2 checks failed",
            CheckTone::Failed,
        );
        card.ci = Some(tab_line(
            "failed: test (ubuntu-latest), clippy",
            CheckTone::Failed,
        ));
        card.buttons = vec![at, at + 1, at + 2, 1];
    }
    view
}

/// `feature_tab_ship_live` once web's dev was pushed and its CI failed:
/// the Base CI lines, and running what failed again.
fn feature_tab_ship_base_ci() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, Dot, PageButton, PageSection};
    let mut view = feature_tab_ship_live();
    (view.page.sections).retain(|s| !matches!(s, PageSection::Recap(_)));
    let at = view.page.buttons.len();
    view.page.buttons.push(PageButton {
        label: "Rerun failed".into(),
        style: ButtonStyle::Plain,
    });
    let web = (view.page.sections.iter())
        .position(|s| matches!(s, PageSection::ShipGroup(g) if g.head.starts_with("web")));
    let web = web.unwrap_or(view.page.sections.len() - 1);
    if let Some(PageSection::ShipGroup(group)) = view.page.sections.get_mut(web) {
        group.steps[1] = ship_ran("Pushed dev to origin", "5m ago \u{b7} 9e1f0c2");
        group.selected = None;
        if let Some(card) = group.card.as_mut() {
            card.dot = Dot::Done;
            card.state = tab_line("merged into dev", CheckTone::Ready);
        }
    }
    let lines = vec![
        tab_line(
            "web: dev at 9e1f0c2 \u{b7} failed: e2e (chromium), typecheck",
            CheckTone::Failed,
        ),
        tab_line(
            "api: main at 3f9c2a1 \u{b7} 4 checks passed",
            CheckTone::Ready,
        ),
    ];
    view.page.sections.insert(
        web + 1,
        PageSection::Lines {
            label: "Base CI".into(),
            lines,
        },
    );
    view.page.sections.insert(
        web + 2,
        PageSection::Toolbar {
            groups: vec![(String::new(), vec![at])],
        },
    );
    view
}

/// `feature_tab_ship_live` once shipping stopped: why, its fix buttons,
/// and web merged into dev here only, its push unticked.
fn feature_tab_ship_stopped() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, Dot, PageButton, PageSection};
    let mut view = feature_tab_ship_live();
    (view.page.sections).retain(|s| !matches!(s, PageSection::Recap(_)));
    let at = view.page.buttons.len();
    for (label, style) in [
        ("Plan fix", ButtonStyle::Primary),
        ("Fix directly", ButtonStyle::Plain),
        ("Push dev\u{2026}", ButtonStyle::Primary),
    ] {
        view.page.buttons.push(PageButton {
            label: label.into(),
            style,
        });
    }
    let web = (view.page.sections.iter())
        .position(|s| matches!(s, PageSection::ShipGroup(g) if g.head.starts_with("web")));
    let web = web.unwrap_or(view.page.sections.len() - 1);
    if let Some(PageSection::ShipGroup(group)) = view.page.sections.get_mut(web)
        && let Some(card) = group.card.as_mut()
    {
        card.dot = Dot::Blocked;
        card.state = tab_line(
            "merged into dev here only: not on origin",
            CheckTone::Blocker,
        );
        card.buttons = vec![at + 2];
    }
    let lines = vec![
        tab_line(
            "Couldn't push dev of api: husky - pre-push hook exited with code 1 \u{b7} 2m ago",
            CheckTone::Failed,
        ),
        tab_line(
            "A hook refused the code: the Build agent fixes it, then Verify and the reviews run again",
            CheckTone::Note,
        ),
    ];
    view.page.sections.insert(
        1,
        PageSection::Lines {
            label: "Shipping stopped".into(),
            lines,
        },
    );
    view.page.sections.insert(
        2,
        PageSection::Toolbar {
            groups: vec![(String::new(), vec![at, at + 1])],
        },
    );
    view
}

/// A done feature, Ship's page shown read-only, with its pull requests.
fn feature_tab_approved() -> super::FeatureTabView {
    feature_tab_brief(false)
}

/// The same brief once the plan is written: it only goes on.
fn feature_tab_locked() -> super::FeatureTabView {
    feature_tab_brief(true)
}

/// Discover's page with the brief approved, `locked` by the plan or not.
fn feature_tab_brief(locked: bool) -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, NodeMark, PageButton, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 1, NodeMark::Current, "not started");
    select_node(&mut view, 0);
    let button = |label: &str, style| PageButton {
        label: label.into(),
        style,
    };
    let mut buttons = vec![button("Go to Plan", ButtonStyle::Primary)];
    if !locked {
        buttons.push(button("Reopen", ButtonStyle::Plain));
    }
    buttons.push(button("Open", ButtonStyle::Plain));
    let (status, state) = match locked {
        true => (
            "Done 1h ago by you \u{b7} locked: Plan has plan.md",
            "approved \u{b7} locked",
        ),
        false => ("Done 1h ago by you", "approved"),
    };
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Discover".into(),
        about: Stage::Discover.about().into(),
        status: Some(tab_line(status, CheckTone::Ready)),
        buttons,
        sections: vec![
            PageSection::File {
                name: "brief.md".into(),
                state: tab_line(state, CheckTone::Ready),
                buttons: match locked {
                    true => vec![1],
                    false => vec![2],
                },
            },
            PageSection::Doc {
                name: "brief.md".into(),
                blocks: std::sync::Arc::new(doc_blocks()),
                here: false,
            },
        ],
    };
    let mut hints = vec![("Left / Right", "stage"), ("Enter", "go to Plan")];
    if !locked {
        hints.push(("Ctrl+Shift+Enter", "reopen"));
    }
    hints.extend([("Tab", "repositories"), ("F1", "keys")]);
    view.hints = (hints.iter())
        .map(|(k, a)| ((*k).to_owned(), (*a).to_owned()))
        .collect();
    view
}

/// Plan at work with its file written: starting it over frees the brief.
fn feature_tab_start_over() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, NodeMark, PageButton, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 1, NodeMark::Waiting, "ready for review");
    let button = |label: &str, style| PageButton {
        label: label.into(),
        style,
    };
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Plan".into(),
        about: Stage::Plan.about().into(),
        status: Some(tab_line("plan.md is ready for review", CheckTone::Blocker)),
        buttons: vec![
            button("Review", ButtonStyle::Primary),
            button("Restart", ButtonStyle::Quiet),
            button("Start over\u{2026}", ButtonStyle::Quiet),
        ],
        sections: vec![
            PageSection::Agent {
                stage: crate::feature::Stage::Plan,
                icon: crate::agent_icons::AgentIcon::Claude,
                name: "claude".into(),
                dot: super::Dot::Idle,
                status: "your turn \u{b7} tab 2".into(),
                tail: Vec::new(),
                buttons: vec![1],
                picks: Vec::new(),
                model: None,
                waits: false,
            },
            PageSection::File {
                name: "plan.md".into(),
                state: tab_line("ready for review", CheckTone::Blocker),
                buttons: vec![0],
            },
            PageSection::Doc {
                name: "plan.md".into(),
                blocks: std::sync::Arc::new(doc_blocks()),
                here: false,
            },
        ],
    };
    let hints = [
        ("Left / Right", "stage"),
        ("Enter", "review"),
        ("Tab", "repositories"),
        ("F1", "keys"),
    ];
    view.hints = (hints.iter())
        .map(|(k, a)| ((*k).to_owned(), (*a).to_owned()))
        .collect();
    view
}

fn feature_tab_done() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, NodeMark, PageButton, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    for node in &mut view.rail {
        node.mark = NodeMark::Done;
        node.line = "2h ago".into();
    }
    view.rail[4].mark = NodeMark::Stale;
    view.rail[4].line = "code changed".into();
    select_node(&mut view, 8);
    for (i, repo) in ["api", "web"].into_iter().enumerate() {
        let row = &mut view.repos[i];
        row.line = format!("PR {repo}/pull/1{i}");
        row.buttons = vec!["Open PR"];
    }
    view.sub = format!("done \u{b7} {}", view.sub);
    view.remove = "Remove worktree and branch";
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Ship".into(),
        about: Stage::Ship.about().into(),
        status: Some(tab_line("Done 2h ago by you", CheckTone::Ready)),
        buttons: vec![PageButton {
            label: "Reopen".into(),
            style: ButtonStyle::Plain,
        }],
        sections: vec![PageSection::Lines {
            label: "Pull requests".into(),
            lines: vec![
                tab_line("api/pull/10 open, 2 checks passed", CheckTone::Ready),
                tab_line("web/pull/11 open, review requested", CheckTone::Note),
            ],
        }],
    };
    view
}

/// Code review current: the rail over all else, its review painted apart
/// (`inline_review_view`), the rail with no focus as the review has the keys.
fn feature_tab_review_page() -> super::FeatureTabView {
    let mut view = feature_tab();
    at_stage(&mut view, 6, super::NodeMark::Waiting, "to review");
    select_node(&mut view, 6);
    view.review_page = true;
    view.review_keys = true;
    view.hints.clear();
    // Its page's bar says the stage, as `features::stage_page` titles it.
    "Code review".clone_into(&mut view.page.title);
    view
}

/// Code review's switch and its figures, in the given view.
fn review_switch(view: super::ReviewView) -> super::ReviewSwitch {
    super::ReviewSwitch {
        view,
        files: Some(super::ReviewFiles {
            viewed: 3,
            total: 8,
            changed: 1,
        }),
        fixes: "1 of 2 made".into(),
        next: None,
        buttons: Vec::new(),
        picks: Vec::new(),
        model: None,
    }
}

/// Code review's actions on its line: Add comment… lit, Plan fixes and
/// Approve in `plan` and `approve`, Approve held up by `why`.
fn review_buttons(
    plan: super::ButtonStyle,
    approve: super::ButtonStyle,
    why: Option<&str>,
) -> Vec<super::ReviewButton> {
    use super::{ButtonStyle, ReposButton as B, ReviewButton};
    let button = |button: B, style, key: &str, disabled: Option<&str>| ReviewButton {
        button,
        label: button.label().into(),
        style,
        key: key.into(),
        disabled: disabled.map(Into::into),
    };
    let plan_why = (plan == ButtonStyle::Off).then_some("Add a comment first");
    vec![
        button(B::AddComment, ButtonStyle::Plain, "Shift+C", None),
        button(B::PlanFixes, plan, "Ctrl+Shift+Enter", plan_why),
        button(B::Approve, approve, "Ctrl+Enter", why),
    ]
}

/// Code review's page on its code: the switch over the review, files left
/// and Approve held up.
fn feature_tab_review_code() -> super::FeatureTabView {
    use super::ButtonStyle;
    let mut view = feature_tab_review_page();
    let mut switch = review_switch(super::ReviewView::Code);
    switch.next = Some((
        "5 of 8 files left \u{b7} 1 changed since viewed".into(),
        false,
    ));
    let why = Some("5 files not viewed: api/src/a.rs, api/src/b.rs, web/c.ts, …");
    switch.buttons = review_buttons(ButtonStyle::Off, ButtonStyle::Off, why);
    // The agent fixes go to, on the bar by the buttons.
    switch.picks = ["Claude Code", "Cursor"]
        .iter()
        .enumerate()
        .map(|(i, name)| super::AgentChip {
            icon: crate::agent_icons::AgentIcon::from_agent(name),
            name: (*name).into(),
            lit: i == 0,
            off: false,
        })
        .collect();
    switch.model = Some("Model: default".into());
    view.review_switch = Some(switch);
    view
}

/// Code review's page with every file viewed and nothing holding it up:
/// Approve is the one to press.
fn feature_tab_review_ready() -> super::FeatureTabView {
    use super::ButtonStyle;
    let mut view = feature_tab_review_page();
    let mut switch = review_switch(super::ReviewView::Code);
    switch.files = Some(super::ReviewFiles {
        viewed: 8,
        total: 8,
        changed: 0,
    });
    let next = "All 8 files viewed \u{b7} Approve to go on to Manual test";
    switch.next = Some((next.into(), true));
    switch.buttons = review_buttons(ButtonStyle::Off, ButtonStyle::Primary, None);
    view.review_switch = Some(switch);
    view
}

/// Code review's page once it is done: the review as it was worked, only to
/// read, with Go to Manual test and Reopen on its line.
fn feature_tab_review_done() -> super::FeatureTabView {
    use super::{ButtonStyle, ReposButton as B, ReviewButton};
    let mut view = feature_tab_review_page();
    at_stage(&mut view, 7, super::NodeMark::Waiting, "to test");
    select_node(&mut view, 6);
    let mut switch = review_switch(super::ReviewView::Code);
    switch.files = Some(super::ReviewFiles {
        viewed: 8,
        total: 8,
        changed: 0,
    });
    switch.fixes = "none".into();
    let next = "Approved \u{b7} only to read: Reopen to change it";
    switch.next = Some((next.into(), true));
    let button = |button: B, label: &str, style, key: &str| ReviewButton {
        button,
        label: label.into(),
        style,
        key: key.into(),
        disabled: None,
    };
    switch.buttons = vec![
        button(
            B::GoOn,
            "Go to Manual test",
            ButtonStyle::Primary,
            "Ctrl+Enter",
        ),
        button(B::Reopen, "Reopen", ButtonStyle::Plain, "Ctrl+Shift+Enter"),
    ];
    view.review_switch = Some(switch);
    view
}

/// Code review's page on its fixes: the plan's card, the comments as cards, an
/// earlier round folded, and the plan to read.
fn feature_tab_review_fixes() -> super::FeatureTabView {
    use super::{
        ButtonStyle, CardStripe, CheckTone, FindingCard, FindingTag, NodeMark, PageButton,
        PageSection, StagePage,
    };
    let mut view = feature_tab();
    at_stage(&mut view, 6, NodeMark::Waiting, "3 of 8 files");
    select_node(&mut view, 6);
    view.review_switch = Some(review_switch(super::ReviewView::Fixes));
    let button = |label: &str, style| PageButton {
        label: label.into(),
        style,
    };
    let card = |state: &str, place: &str, title: &str, made, stripe, buttons: Vec<usize>| {
        PageSection::Finding(FindingCard {
            sheet: None,
            tag: FindingTag::Yours,
            title: title.into(),
            state: state.into(),
            place: place.into(),
            body: String::new(),
            preview: Vec::new(),
            fix: String::new(),
            note: String::new(),
            comment: String::new(),
            comment_buttons: Vec::new(),
            head: 0,
            made,
            faded: false,
            collapsed: false,
            folds: false,
            stripe,
            field: None,
            field_hint: "",
            clock: String::new(),
            selected: false,
            buttons,
        })
    };
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Code review".into(),
        about: "Read the change and comment on it".into(),
        status: None,
        buttons: vec![
            button("Review", ButtonStyle::Primary),
            button("Make fixes", ButtonStyle::Plain),
            button("Drop plan\u{2026}", ButtonStyle::Quiet),
            button("Edit\u{2026}", ButtonStyle::Plain),
            button("Delete", ButtonStyle::Plain),
            button("Show in diff", ButtonStyle::Quiet),
            button("Show in diff", ButtonStyle::Quiet),
            button("Show in diff", ButtonStyle::Quiet),
            button("Show", ButtonStyle::Plain),
        ],
        sections: vec![
            PageSection::Lines {
                label: "Fix rounds \u{b7} 2".into(),
                lines: Vec::new(),
            },
            PageSection::Round(super::RoundCard {
                label: fix_label(2),
                dot: super::Dot::Idle,
                state: tab_line("review comments \u{b7} fix plan ready", CheckTone::Blocker),
                meta: "asked by Code review 6m ago \u{b7} plan written 2m ago".into(),
                asked: vec![
                    super::RoundAsked {
                        id: "C1".into(),
                        title: "src/login.rs:12".into(),
                        note: "Rename this to say it retries".into(),
                        now: None,
                    },
                    super::RoundAsked {
                        id: "C2".into(),
                        title: "src/retry.rs:40".into(),
                        note: "Add a test for the last attempt".into(),
                        now: None,
                    },
                ],
                asked_caption: "Asked to fix",
                fixes: Vec::new(),
                running: Vec::new(),
                changed: Vec::new(),
                open: true,
                nested: false,
                buttons: vec![0, 1, 2],
            }),
            PageSection::Round(super::RoundCard {
                label: fix_label(1),
                dot: super::Dot::Done,
                state: tab_line("2 of 2 made", CheckTone::Ready),
                meta: "asked by Code review 1h ago \u{b7} made by the Build agent (Claude Code) \
                       \u{b7} finished 40m ago"
                    .into(),
                asked: Vec::new(),
                asked_caption: "Asked to fix",
                fixes: Vec::new(),
                running: Vec::new(),
                changed: Vec::new(),
                open: false,
                nested: false,
                buttons: vec![8],
            }),
            PageSection::Lines {
                label: "Comments \u{b7} 1 of 2 made".into(),
                lines: Vec::new(),
            },
            card(
                "goes with the fixes",
                "src/new.rs:3",
                "This should say what it waits for",
                false,
                None,
                vec![3, 4, 5],
            ),
            card(
                crate::feature::review_import::sent_word(false, true),
                "C1 \u{b7} src/login.rs:12",
                "Rename this to say it retries",
                false,
                Some(CardStripe::Sent),
                vec![6],
            ),
            card(
                "fix made",
                "C2 \u{b7} src/retry.rs:40",
                "Add a test for the last attempt",
                true,
                Some(CardStripe::Sent),
                vec![7],
            ),
            PageSection::Doc {
                name: "fix-plan.md".into(),
                blocks: std::sync::Arc::new(crate::md_blocks::blocks(
                    "## Fixes\n\n### C1: Rename the loop\n\nIts name hides that it retries.\n\n\
                     ### C2: Test the last attempt\n\nNothing checks the limit.\n",
                )),
                here: false,
            },
        ],
    };
    view
}

/// Build current, a later stage's page picked: no buttons.
fn feature_tab_later() -> super::FeatureTabView {
    use super::{CheckTone, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    select_node(&mut view, 6);
    view.hover = Some(super::FeatureTabHit::Node(7));
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Code review".into(),
        about: Stage::CodeReview.about().into(),
        status: None,
        buttons: Vec::new(),
        sections: vec![PageSection::Lines {
            label: String::new(),
            lines: vec![tab_line("Comes after AI review", CheckTone::Note)],
        }],
    };
    view
}

/// The chips of an agent card: Claude Code, Cursor, which `lit`, and
/// whether Claude Code built the code.
fn agent_chips(lit: usize, off: bool) -> Vec<super::AgentChip> {
    ["Claude Code", "Cursor"]
        .iter()
        .enumerate()
        .map(|(i, name)| super::AgentChip {
            icon: crate::agent_icons::AgentIcon::from_agent(name),
            name: (*name).into(),
            lit: i == lit,
            off: off && i == 0,
        })
        .collect()
}

/// AI review not reached yet: only the chips, the builder's off.
fn feature_tab_picks_later() -> super::FeatureTabView {
    use super::{CheckTone, Dot, PageSection, StagePage};
    use crate::{agent_icons::AgentIcon, feature::Stage};
    let mut view = feature_tab();
    select_node(&mut view, 5);
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "AI review".into(),
        about: Stage::AiReview.about().into(),
        status: Some(tab_line("Comes after Verify", CheckTone::Note)),
        buttons: Vec::new(),
        sections: vec![PageSection::Agent {
            stage: crate::feature::Stage::AiReview,
            icon: AgentIcon::Generic,
            name: String::new(),
            dot: Dot::Unknown,
            status: String::new(),
            tail: Vec::new(),
            buttons: Vec::new(),
            picks: agent_chips(1, true),
            model: None,
            waits: false,
        }],
    };
    view
}

/// Build running with claude, Cursor picked for its restart, the pointer
/// on Claude Code's chip.
fn feature_tab_picks_running() -> super::FeatureTabView {
    use super::PageSection;
    let mut view = feature_tab();
    for section in &mut view.page.sections {
        if let PageSection::Agent { picks, .. } = section {
            *picks = agent_chips(1, false);
        }
    }
    if let Some(restart) = (view.page.buttons.iter_mut()).find(|b| b.label == "Restart") {
        restart.label = "Restart with Cursor".into();
    }
    view.hover = Some(super::FeatureTabHit::AgentPick(
        crate::feature::Stage::Build,
        0,
    ));
    view
}

/// `view` with the agent card's model button saying `label`, the pointer on it.
fn with_model_button(mut view: super::FeatureTabView, label: &str) -> super::FeatureTabView {
    for section in &mut view.page.sections {
        if let super::PageSection::Agent { stage, model, .. } = section {
            *model = Some(label.into());
            view.hover = Some(super::FeatureTabHit::AgentModel(*stage));
        }
    }
    view
}

/// Build set to a fresh agent per milestone, the pointer on the other choice.
fn feature_tab_milestone_mode() -> super::FeatureTabView {
    use super::PageSection;
    let mut view = feature_tab();
    let after = (view.page.sections.iter())
        .position(|s| matches!(s, PageSection::Agent { .. }))
        .map_or(0, |i| i + 1);
    view.page.sections.insert(
        after,
        PageSection::MilestoneMode {
            fresh: true,
            fixes: false,
        },
    );
    view.hover = Some(super::FeatureTabHit::MilestoneMode(0));
    view
}

/// Plan's agent to run in T3 Code, its draft waiting to be sent.
/// An agent open on Plan, waiting for its model to be picked and its prompt
/// sent.
fn feature_tab_held() -> super::FeatureTabView {
    use super::{CheckTone, Dot, PageButton, PageSection, StagePage};
    use crate::{agent_icons::AgentIcon, chrome::ButtonStyle, feature::Stage};
    let mut view = feature_tab();
    select_node(&mut view, 1);
    let picks = ["Claude Code", "T3 Code", "Cursor"]
        .iter()
        .enumerate()
        .map(|(i, name)| super::AgentChip {
            icon: AgentIcon::from_agent(name),
            name: (*name).into(),
            lit: i == 0,
            off: false,
        })
        .collect();
    let button = |label: &str, style| PageButton {
        label: label.into(),
        style,
    };
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Plan".into(),
        about: Stage::Plan.about().into(),
        status: Some(tab_line("not written yet", CheckTone::Note)),
        buttons: vec![
            button("Send prompt", ButtonStyle::Primary),
            button("Show agent", ButtonStyle::Plain),
        ],
        sections: vec![PageSection::Agent {
            stage: crate::feature::Stage::Plan,
            icon: AgentIcon::from_agent("Claude Code"),
            name: "Claude Code".into(),
            dot: Dot::Blocked,
            status: "your turn \u{b7} send the prompt".into(),
            tail: Vec::new(),
            buttons: vec![0, 1],
            picks,
            model: None,
            waits: true,
        }],
    };
    view
}

/// On AI review with the code changed since: the last review to look at,
/// and the earlier ones in the history under it, the oldest shown.
fn feature_tab_review_rounds() -> super::FeatureTabView {
    use super::{
        ButtonStyle, CheckTone, FindingCard, FindingTag, NodeMark, PageButton, PageSection,
        StagePage,
    };
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 5, NodeMark::Later, "");
    let button = |label: &str, style| PageButton {
        label: label.into(),
        style,
    };
    let card = |tag, title: &str, state: &str, place: &str, fix: &str, faded, buttons| {
        PageSection::Finding(FindingCard {
            sheet: None,
            tag,
            title: title.into(),
            state: state.into(),
            place: place.into(),
            body: String::new(),
            preview: Vec::new(),
            fix: fix.into(),
            note: String::new(),
            comment: String::new(),
            comment_buttons: Vec::new(),
            head: 0,
            made: state == "fix made",
            faded,
            collapsed: false,
            folds: false,
            stripe: None,
            field: None,
            field_hint: "",
            clock: String::new(),
            selected: false,
            buttons,
        })
    };
    let round = |name: &str, said: &str, button: usize| PageSection::File {
        name: name.into(),
        state: tab_line(said, CheckTone::Note),
        buttons: vec![button],
    };
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "AI review".into(),
        about: Stage::AiReview.about().into(),
        status: Some(tab_line("Comes after Verify", CheckTone::Note)),
        buttons: vec![
            button("Show in diff", ButtonStyle::Quiet),
            button("Show", ButtonStyle::Plain),
            button("Hide", ButtonStyle::Plain),
        ],
        sections: vec![
            PageSection::Review(super::ReviewSummary {
                label: "Findings".into(),
                counts: vec![(FindingTag::High, 1), (FindingTag::Medium, 1)],
                open: vec![0, 0],
                decided: 2,
                refused: 0,
                total: 2,
                words: "1 being fixed \u{b7} 1 fix made".into(),
                notes: vec![tab_line(
                    "From the last review. The code has changed since.",
                    CheckTone::Note,
                )],
                buttons: Vec::new(),
            }),
            card(
                FindingTag::High,
                "Retry stops one short",
                "being fixed",
                "r2-1 \u{b7} login.py:6",
                "Fix: Stop at n.",
                false,
                vec![0],
            ),
            card(
                FindingTag::Medium,
                "Empty name logs in",
                "fix made",
                "r2-2 \u{b7} login.py:9",
                "Fix: Refuse an empty name.",
                false,
                Vec::new(),
            ),
            PageSection::Lines {
                label: "History".into(),
                lines: Vec::new(),
            },
            round("AI review run 2", "2 fixed", 1),
            round(
                "AI review run 1",
                "1 fixed \u{b7} 1 raised again \u{b7} 1 dismissed",
                2,
            ),
            card(
                FindingTag::Medium,
                "Title breaks with no name",
                "raised again",
                "r1-2 \u{b7} login.py:2",
                "Your fix: Fall back to the plain title",
                true,
                Vec::new(),
            ),
            card(
                FindingTag::Low,
                "Unused import",
                "fixed",
                "r1-1 \u{b7} login.py:1",
                "",
                true,
                Vec::new(),
            ),
        ],
    };
    view
}

/// At AI review, its findings sent to the Build agent, which runs again.
fn feature_tab_fixed_code() -> super::FeatureTabView {
    use super::NodeMark;
    let mut view = feature_tab();
    at_stage(&mut view, 3, NodeMark::Current, "working 2m");
    for node in &mut view.rail[4..=5] {
        node.mark = NodeMark::Ran;
        node.line = "1h ago".into();
    }
    turned_back(&mut view, 5);
    view
}

/// AI review's findings sent back to the Build agent, running on them.
fn feature_tab_fixing() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, Dot, NodeMark, PageButton, PageSection, StagePage};
    use crate::{agent_icons::AgentIcon, feature::Stage};
    let mut view = feature_tab();
    at_stage(&mut view, 5, NodeMark::Paused, "making fixes");
    view.rail[3].mark = NodeMark::Fixing;
    view.rail[3].line = "working again".into();
    turned_back(&mut view, 5);
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "AI review".into(),
        about: Stage::AiReview.about().into(),
        status: None,
        buttons: vec![PageButton {
            label: "Show agent".into(),
            style: ButtonStyle::Plain,
        }],
        sections: vec![
            PageSection::Agent {
                stage: crate::feature::Stage::Build,
                icon: AgentIcon::Claude,
                name: "claude".into(),
                dot: Dot::Working,
                status: "Build agent \u{b7} working again".into(),
                tail: Vec::new(),
                buttons: vec![0],
                picks: Vec::new(),
                model: None,
                waits: false,
            },
            PageSection::MilestoneMode {
                fresh: false,
                fixes: true,
            },
            super::PageSection::Lines {
                label: "Findings".into(),
                lines: vec![tab_line("2 being fixed", CheckTone::Note)],
            },
        ],
    };
    view
}

/// AI review at work, Build done: the reviewer's card, then the card of the
/// agent fixes go to, each with its own chips, the fixes' with its model.
pub(super) fn feature_tab_fix_picks() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, Dot, NodeMark, PageButton, PageSection, StagePage};
    use crate::{agent_icons::AgentIcon, feature::Stage};
    let mut view = feature_tab();
    at_stage(&mut view, 5, NodeMark::Current, "reviewing");
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "AI review".into(),
        about: Stage::AiReview.about().into(),
        status: None,
        buttons: vec![PageButton {
            label: "Show agent".into(),
            style: ButtonStyle::Plain,
        }],
        sections: vec![
            PageSection::Agent {
                stage: Stage::AiReview,
                icon: AgentIcon::Cursor,
                name: "Cursor".into(),
                dot: Dot::Working,
                status: "working \u{b7} tab 3".into(),
                tail: Vec::new(),
                buttons: vec![0],
                picks: agent_chips(1, true),
                model: None,
                waits: false,
            },
            PageSection::Agent {
                stage: Stage::Build,
                icon: AgentIcon::Claude,
                name: "Claude Code".into(),
                dot: Dot::Done,
                status: "Build agent \u{b7} next fixes: a fresh Claude Code".into(),
                tail: Vec::new(),
                buttons: Vec::new(),
                picks: agent_chips(0, false),
                model: Some("Model: default".into()),
                waits: false,
            },
            PageSection::Lines {
                label: "Findings".into(),
                lines: vec![tab_line("none yet", CheckTone::Note)],
            },
        ],
    };
    view
}

/// Ship with nothing ticked, its card picking the agent that writes the
/// summary and resolves conflicts.
fn feature_tab_ship_picks() -> super::FeatureTabView {
    use super::{Dot, PageSection};
    use crate::{agent_icons::AgentIcon, feature::Stage};
    let mut view = feature_tab_ship_finish();
    view.page.sections.insert(
        0,
        PageSection::Agent {
            stage: Stage::Ship,
            icon: AgentIcon::Generic,
            name: String::new(),
            dot: Dot::Unknown,
            status: String::new(),
            tail: Vec::new(),
            buttons: Vec::new(),
            picks: agent_chips(0, false),
            model: Some("Model: default".into()),
            waits: false,
        },
    );
    view
}

fn feature_tab_t3_draft() -> super::FeatureTabView {
    use super::{CheckTone, Dot, PageSection, StagePage};
    use crate::{agent_icons::AgentIcon, feature::Stage};
    let mut view = feature_tab();
    select_node(&mut view, 1);
    let picks = ["Claude Code", "T3 Code", "Cursor"]
        .iter()
        .enumerate()
        .map(|(i, name)| super::AgentChip {
            icon: AgentIcon::from_agent(name),
            name: (*name).into(),
            lit: i == 1,
            off: false,
        })
        .collect();
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Plan".into(),
        about: Stage::Plan.about().into(),
        status: Some(tab_line("not written yet", CheckTone::Note)),
        buttons: Vec::new(),
        sections: vec![PageSection::Agent {
            stage: crate::feature::Stage::Plan,
            icon: AgentIcon::T3,
            name: "T3 Code".into(),
            dot: Dot::Blocked,
            status: "draft in T3 Code \u{b7} pick a model and send it".into(),
            tail: Vec::new(),
            buttons: Vec::new(),
            picks,
            model: None,
            waits: true,
        }],
    };
    view
}

/// The rows unfolded, by group, with what each can become.
fn feature_tab_unfolded() -> super::FeatureTabView {
    use super::{FeatureRepoRow, RepoKind, RepoNote};
    let mut view = feature_tab();
    view.expanded = true;
    let offered = |name: &str, line: &str, group: Option<&str>| FeatureRepoRow {
        name: name.into(),
        line: line.into(),
        kind: RepoKind::Other,
        group: group.map(str::to_owned),
        busy: false,
        buttons: vec!["Feature", "Context"],
        remove: false,
        stat: None,
    };
    view.repos[0].group = Some("In feature".into());
    view.repos[0].buttons = vec!["Context"];
    view.repos[1].buttons = vec!["Open PR", "Context"];
    view.repos[2].group = Some("Context, read only".into());
    view.repos[2].line = "~/Work/aft/shared".into();
    view.repos[2].buttons = vec!["Feature"];
    view.repos.extend([
        offered("auth", "removed 2h ago", Some("Removed")),
        offered("infra", "main", Some("Not added")),
        offered("billing", "develop", None),
    ]);
    view.repo_notes = vec![RepoNote {
        caption: "tools".into(),
        text: "couldn't read: no such folder".into(),
        error: true,
    }];
    view
}

/// The keyboard on the repositories' rows, on shared with its Feature
/// button picked, the pointer on a row's other button.
fn feature_tab_repos() -> super::FeatureTabView {
    let mut view = feature_tab_unfolded();
    view.focus = super::TabArea::Repos;
    view.repo = 2;
    view.picked = Some(0);
    view.hover = Some(super::FeatureTabHit::RepoButton(3, 1));
    view.hints[1] = ("Up / Down".into(), "repository".into());
    view.hints[2] = ("Enter".into(), "feature button".into());
    view
}

/// A shipped feature unfolded: its rows, and nothing to do on them.
fn feature_tab_shipped() -> super::FeatureTabView {
    let mut view = feature_tab_done();
    view.expanded = true;
    view.more = 0;
    for row in &mut view.repos {
        row.buttons.clear();
        row.remove = false;
    }
    view.repos[0].group = Some("In feature".into());
    view.repos[2].group = Some("Context, read only".into());
    view
}

/// A project's repositories couldn't be listed: said under the rows, not
/// taken for it having none.
fn feature_tab_repos_failed() -> super::FeatureTabView {
    let mut view = feature_tab();
    view.expanded = true;
    view.repos[0].group = Some("In feature".into());
    view.repos[2].group = Some("Context, read only".into());
    view.repo_notes = vec![
        super::RepoNote {
            caption: "tools".into(),
            text: "couldn't read: no such folder".into(),
            error: true,
        },
        super::RepoNote {
            caption: "admin".into(),
            text: "looking\u{2026}".into(),
            error: false,
        },
    ];
    view
}

/// The repositories couldn't be read.
fn feature_tab_repos_error() -> super::FeatureTabView {
    let mut view = feature_tab();
    view.repos_error = Some("git status in api: fatal: not a git repository".into());
    view
}

/// Eleven repositories, more than a narrow tab's strip holds: from the
/// first, the pointer on the "+N" chip standing for those after.
fn feature_tab_more_repos() -> super::FeatureTabView {
    let mut view = feature_tab();
    let row = view.repos[0].clone();
    view.repos = [
        "api",
        "web-frontend",
        "shared",
        "billing-service",
        "search-index",
        "mobile-app",
        "infra",
        "docs-site",
        "auth",
        "analytics-pipeline",
        "payments-gateway",
    ]
    .map(|name| super::FeatureRepoRow {
        name: name.into(),
        ..row.clone()
    })
    .to_vec();
    let l = super::feature_tab_layout(
        &view,
        smithay::utils::Rectangle::from_size((850, TAB_HEIGHT).into()),
    );
    view.hover = l
        .more_repos
        .last()
        .map(|(_, i, _)| super::FeatureTabHit::MoreRepos(*i));
    view
}

/// The keyboard on a repository past the strip's first chips: it starts
/// at that one, after "N+" for those before.
fn feature_tab_more_repos_mid() -> super::FeatureTabView {
    let mut view = feature_tab_more_repos();
    view.hover = None;
    view.focus = super::TabArea::Repos;
    view.repo = 9;
    view
}

/// So narrow the longer stages' labels take two lines; web being
/// touched (its chip's dot blinks), shared adding lines only.
fn feature_tab_rail_wrap() -> super::FeatureTabView {
    let mut view = feature_tab();
    view.repos[1].busy = true;
    view.repos[2].stat = Some(Stat {
        files: 2,
        added: 7,
        removed: 0,
    });
    at_stage(&mut view, 6, super::NodeMark::Current, "3 of 9 viewed");
    view
}

/// Manual test's page: a criterion of each kind, one long enough to wrap, a
/// fix made to try again, a failure the next fixes send; `note`: a failure's note being written on its card.
fn feature_tab_manual_test(note: bool) -> super::FeatureTabView {
    use super::{
        ButtonStyle, CheckTone, FindingCard, FindingTag, NodeMark, PageButton, PageSection,
        StagePage,
    };
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 7, NodeMark::Current, "1 of 4 passed");
    let button = |label: &str, style| PageButton {
        label: label.into(),
        style,
    };
    // Mark passed greyed while criteria are left, last before the primary.
    let mut buttons = vec![
        button("Plan fixes", ButtonStyle::Primary),
        button("Start sandbox\u{2026}", ButtonStyle::Plain),
        button("Mark passed", ButtonStyle::Off),
    ];
    let (pass, fail, plain, quiet) = (
        ButtonStyle::Passed,
        ButtonStyle::Failed,
        ButtonStyle::Plain,
        ButtonStyle::Quiet,
    );
    // Its buttons, how it went, and its title, state, id, note and steps.
    type Card<'a> = (&'a [(&'a str, ButtonStyle)], FindingTag, [&'a str; 5]);
    let cards: [Card; 4] = [
        (
            &[("Pass", pass), ("Fail\u{2026}", plain)],
            FindingTag::Passed,
            [
                "Export holds both invoices",
                "",
                "AC-1",
                "",
                "Given two invoices\nWhen I export both\nThen one CSV holds both",
            ],
        ),
        (
            &[
                ("Still wrong\u{2026}", quiet),
                ("Pass", plain),
                ("Fail\u{2026}", fail),
            ],
            FindingTag::Failed,
            [
                "Export off with no invoices",
                "fix made: try again",
                "AC-2",
                "What went wrong: The button stays on and exports an empty file",
                "Given no invoices\nWhen I export\nThen the button is off",
            ],
        ),
        (
            &[
                ("Edit note\u{2026}", quiet),
                ("Pass", plain),
                ("Fail", fail),
            ],
            FindingTag::Failed,
            [
                "Cancelled invoices left out",
                "to fix",
                "AC-3",
                "What went wrong: It shows with a total of 0",
                "Given a cancelled invoice\nWhen I export\nThen it is left out of the file and \
                 of the total at the bottom of the page, and the count above the table goes \
                 down by one",
            ],
        ),
        (
            &[
                ("Pass", plain),
                ("Fail\u{2026}", if note { fail } else { plain }),
            ],
            FindingTag::NotRun,
            [
                "Export takes a second",
                "",
                "AC-4",
                "",
                "Given 500 invoices\nWhen I export\nThen it is done in a second",
            ],
        ),
    ];
    // The row folding them, at the caption's right, as the page has it.
    let fold = crate::features::fold_actions();
    let PageSection::Toolbar { groups } = crate::features::fold_toolbar(&fold) else {
        unreachable!()
    };
    let at = buttons.len();
    let groups = (groups.into_iter())
        .map(|(word, held)| (word, held.iter().map(|i| at + i).collect()))
        .collect();
    buttons.extend(fold.into_iter().map(|(b, _)| b));
    let mut sections = vec![
        PageSection::Lines {
            label: "Criteria".into(),
            lines: vec![tab_line("uncommitted changes in api", CheckTone::Note)],
        },
        PageSection::Toolbar { groups },
    ];
    for (i, (labels, tag, [title, state, place, said, steps])) in cards.into_iter().enumerate() {
        let at = buttons.len();
        buttons.extend(labels.iter().map(|(l, style)| button(l, *style)));
        let writing = note && i == 3;
        let selected = i == if note { 3 } else { 2 };
        // The one passed folded to its line, as passing left it.
        let open = tag != FindingTag::Passed;
        let steps = steps
            .lines()
            .map(|l| {
                let (word, rest) = l.split_once(' ').unwrap_or(("", l));
                let word = ["Given", "When", "Then"].into_iter().find(|w| *w == word);
                (word.unwrap_or(""), rest.to_owned())
            })
            .collect();
        sections.push(PageSection::Finding(FindingCard {
            sheet: Some(Box::new(super::SheetRow {
                open,
                steps,
                hidden: false,
                needle: String::new(),
                whole: false,
                shots: Vec::new(),
            })),
            tag,
            title: title.into(),
            state: state.into(),
            place: place.into(),
            body: String::new(),
            preview: Vec::new(),
            fix: String::new(),
            note: said.into(),
            comment: String::new(),
            comment_buttons: Vec::new(),
            head: 0,
            made: i == 1,
            faded: tag == FindingTag::Passed,
            collapsed: !open,
            folds: false,
            // Not sent yet: the next fixes send it.
            stripe: (i == 2).then_some(super::CardStripe::ToFix),
            field: writing.then(|| Line::from("Takes 4 seconds")),
            field_hint: "",
            clock: String::new(),
            selected,
            buttons: (at..at + labels.len()).collect(),
        }));
    }
    view.page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: Stage::ManualTest.label().into(),
        about: Stage::ManualTest.about().into(),
        status: None,
        buttons,
        sections,
    };
    // Its counts go in the bar over the page.
    view.test_strip = Some(super::TestStrip {
        search: None,
        shown: None,
        passed: 1,
        failed: 2,
        untested: 1,
        text: "1 passed \u{b7} 2 failed \u{b7} 1 untested \u{b7} 1 fix made \u{b7} 1 to fix".into(),
    });
    if note {
        // Placed before the note's Save and Discard join its card, as the
        // page does; Save is then the page's primary, on its bar.
        crate::features::place_buttons(&mut view.page);
        let at = view.page.buttons.len();
        view.page.buttons.extend([
            button("Discard", ButtonStyle::Quiet),
            button("Save", ButtonStyle::Primary),
        ]);
        if let Some(PageSection::Finding(card)) = view.page.sections.last_mut() {
            card.buttons.extend([at, at + 1]);
        }
        crate::features::fill_only(&mut view.page.buttons, &mut view.page.primary, at + 1);
        view.hints = [
            ("Enter", "save"),
            ("Shift+Enter", "new line"),
            (crate::text_field::DISCARD_KEY, "discard"),
            ("Esc", "keep draft"),
        ]
        .iter()
        .map(|(k, w)| ((*k).to_owned(), (*w).to_owned()))
        .collect();
    }
    view
}

/// Manual test's page with a comment of the user's own after the criteria,
/// open and selected, written on an area of the app's page: Make fixes and
/// Plan fixes over them, Add issue….
fn feature_tab_manual_comment() -> super::FeatureTabView {
    use super::{ButtonStyle, FindingCard, FindingTag, PageButton, PageSection};
    let mut view = feature_tab_manual_test(false);
    let button = |label: &str, style| PageButton {
        label: label.into(),
        style,
    };
    let page = &mut view.page;
    page.buttons[0] = button("Make fixes", ButtonStyle::Primary);
    page.buttons
        .insert(1, button("Plan fixes", ButtonStyle::Plain));
    page.buttons
        .insert(3, button("Add issue\u{2026}", ButtonStyle::Plain));
    // What the cards and the toolbar name moved on by the two.
    for section in &mut page.sections {
        let ids = match section {
            // Folded, so the comment shows.
            PageSection::Finding(card) => {
                card.selected = false;
                card.collapsed = true;
                if let Some(sheet) = card.sheet.as_mut() {
                    sheet.open = false;
                }
                &mut card.buttons
            }
            PageSection::Toolbar { groups } => {
                for (_, ids) in groups.iter_mut() {
                    ids.iter_mut().for_each(|b| *b += 2);
                }
                continue;
            }
            _ => continue,
        };
        ids.iter_mut().for_each(|b| *b += 2);
    }
    let at = page.buttons.len();
    page.buttons.extend([
        button("Edit\u{2026}", ButtonStyle::Quiet),
        button("Delete", ButtonStyle::Quiet),
        button("Done", ButtonStyle::Plain),
        button("Not done", ButtonStyle::Failed),
    ]);
    page.sections.push(PageSection::Finding(FindingCard {
        sheet: Some(Box::new(super::SheetRow {
            open: true,
            steps: vec![
                ("", "Make the settings page look better".to_owned()),
                ("", "Cards for each group, not one long table".to_owned()),
            ],
            hidden: false,
            needle: String::new(),
            whole: false,
            shots: golden_shots(),
        })),
        tag: FindingTag::Failed,
        title: "Make the settings page look better\u{2026}".into(),
        state: "to fix".into(),
        place: "C1".into(),
        // Written on an area selected on the app's page.
        body: "On /settings: SettingsTable and 2 more".into(),
        preview: Vec::new(),
        fix: String::new(),
        note: String::new(),
        comment: String::new(),
        comment_buttons: Vec::new(),
        head: 0,
        made: false,
        faded: false,
        collapsed: false,
        folds: false,
        stripe: Some(super::CardStripe::ToFix),
        field: None,
        field_hint: "",
        clock: String::new(),
        selected: true,
        buttons: (at..at + 4).collect(),
    }));
    if let Some(strip) = view.test_strip.as_mut() {
        strip.failed = 3;
        strip.text =
            "1 passed \u{b7} 2 failed \u{b7} 1 untested \u{b7} 1 issue open \u{b7} 1 fix made \u{b7} 2 to fix"
                .into();
    }
    view
}

/// Manual test's page searching its criteria for "total": only the one
/// saying it shows, open, the places tinted, the field with the keys.
fn feature_tab_manual_search() -> super::FeatureTabView {
    use super::{PageSection, SearchBar};
    let mut view = feature_tab_manual_test(false);
    let needle = "total";
    for section in &mut view.page.sections {
        if let PageSection::Finding(card) = section
            && let Some(sheet) = card.sheet.as_mut()
        {
            let says = sheet.steps.iter().any(|(_, t)| t.contains(needle));
            sheet.hidden = !says;
            sheet.needle = needle.into();
            card.collapsed = !says;
            card.selected = says;
        }
    }
    if let Some(strip) = view.test_strip.as_mut() {
        strip.search = Some(SearchBar {
            query: Line::from(needle),
            placeholder: super::CRITERIA_SEARCH_HINT.into(),
            count: "1 of 2".into(),
            found: true,
            focused: true,
            whole: false,
        });
        strip.shown = Some("showing 1 of 4 criteria saying \u{201c}total\u{201d}".into());
    }
    view.marks.found_at = Some(0);
    view.hints = [("Enter", "to the criteria"), ("Esc", "clear")]
        .iter()
        .map(|(k, w)| ((*k).to_owned(), (*w).to_owned()))
        .collect();
    view
}

/// Manual test's page with text selected across an open criterion's
/// lines, from its title into its steps.
fn feature_tab_manual_select() -> super::FeatureTabView {
    use super::{TextPick, TextSpot};
    let mut view = placed(feature_tab_manual_test(false));
    let frame = smithay::utils::Rectangle::from_size((860, TAB_HEIGHT).into());
    let layout = super::feature_tab_layout(&view, frame);
    let first = (layout.texts.iter())
        .position(|t| t.card.is_some_and(|c| c > 0))
        .expect("a criterion's line");
    let (anchor, cursor) = (
        TextSpot { line: first, ch: 4 },
        TextSpot {
            line: first + 3,
            ch: 9,
        },
    );
    let said = |s: TextSpot| layout.line_text(s.line).expect("a line").to_owned();
    view.marks.pick = Some(TextPick {
        anchor,
        cursor,
        said: [said(anchor), said(cursor)],
    });
    view.hints = [
        ("Ctrl+C / y", "copy"),
        ("* / #", "search it"),
        ("Esc", "unselect"),
    ]
    .iter()
    .map(|(k, w)| ((*k).to_owned(), (*w).to_owned()))
    .collect();
    view
}

#[test]
fn the_page_text_under_the_pointer_selects_and_copies_as_it_reads() {
    use super::{Piece, TabColumn, TextPick, TextSpot};
    let mut view = feature_tab_manual_test(false);
    let frame = smithay::utils::Rectangle::from_size((860, TAB_HEIGHT).into());
    let layout = super::feature_tab_layout(&view, frame);
    // AC-2 is open: its title, its id line, then "Given" beside its step.
    let line = (layout.texts.iter())
        .position(|t| t.card == Some(1))
        .expect("AC-2's title");
    let text = |n: usize| layout.line_text(n).expect("a line").to_owned();
    assert_eq!(text(line), "Export off with no invoices");
    assert_eq!(text(line + 2), "Given");
    let rect = |n: usize| layout.pieces[TabColumn::Page as usize][layout.texts[n].piece].0;
    let start = rect(line);
    let at = |x: i32, y: i32| layout.text_at(&view, (x, y).into(), false);
    let mid = start.loc.y + start.size.h / 2;
    assert_eq!(at(start.loc.x + 1, mid), Some(TextSpot { line, ch: 0 }));
    // Off the page's text: nothing, unless near.
    assert_eq!(at(start.loc.x - 40, mid), None);
    let near = layout.text_at(&view, (start.loc.x - 40, mid).into(), true);
    assert_eq!(near, Some(TextSpot { line, ch: 0 }));
    // From the title into the step beside "Given": lines side by side
    // joined by a space, others by a new line; backwards the same.
    let pick = |anchor: TextSpot, cursor: TextSpot| TextPick {
        anchor,
        cursor,
        said: [text(anchor.line), text(cursor.line)],
    };
    let (from, to) = (
        TextSpot { line, ch: 7 },
        TextSpot {
            line: line + 3,
            ch: 2,
        },
    );
    view.marks.pick = Some(pick(to, from));
    let copied = layout.picked_text(&view).expect("text selected");
    assert_eq!(
        copied,
        format!("off with no invoices\n{}\nGiven no", text(line + 1))
    );
    // Painted: a ground under each line it takes.
    let laid = super::feature_tab_layout(&view, frame);
    let grounds = laid.pieces[TabColumn::Page as usize]
        .iter()
        .filter(|(_, p)| matches!(p, Piece::Picked { .. }))
        .count();
    assert_eq!(grounds, 4);
    // Once the page says something else there, it is gone.
    let mut gone = pick(from, to);
    gone.said[0] = "Something else".into();
    view.marks.pick = Some(gone);
    assert_eq!(layout.picked_text(&view), None);
    let laid = super::feature_tab_layout(&view, frame);
    let pieces = &laid.pieces[TabColumn::Page as usize];
    assert!(
        !pieces
            .iter()
            .any(|(_, p)| matches!(p, Piece::Picked { .. }))
    );
}

#[test]
fn the_match_gone_to_is_the_one_tinted_more() {
    use super::{Piece, TabColumn};
    let mut view = feature_tab_manual_search();
    let frame = smithay::utils::Rectangle::from_size((860, TAB_HEIGHT).into());
    let currents = |view: &super::FeatureTabView| -> Vec<Option<usize>> {
        let layout = super::feature_tab_layout(view, frame);
        (layout.pieces[TabColumn::Page as usize].iter())
            .filter_map(|(_, p)| match p {
                Piece::Found { current, .. } => Some(*current),
                _ => None,
            })
            .collect()
    };
    let layout = super::feature_tab_layout(&view, frame);
    assert_eq!(layout.matches.len(), 2);
    assert_eq!(currents(&view), [Some(0), None]);
    view.marks.found_at = Some(1);
    assert_eq!(currents(&view), [None, Some(0)]);
}

/// Manual test's page with its sandbox running, its app with three pages:
/// a button for each past the first, all at the title.
fn feature_tab_manual_pages() -> super::FeatureTabView {
    use super::{ButtonStyle, PageButton};
    let mut view = feature_tab_manual_test(false);
    let button = |label: &str, style| PageButton {
        label: label.into(),
        style,
    };
    view.page.buttons[2] = button("Open app", ButtonStyle::Plain);
    for (label, style) in [
        ("Django admin", ButtonStyle::Quiet),
        ("Expo web", ButtonStyle::Quiet),
        ("Open to network\u{2026}", ButtonStyle::Quiet),
        ("Restart", ButtonStyle::Quiet),
        ("Reset data\u{2026}", ButtonStyle::Quiet),
        ("Stop\u{2026}", ButtonStyle::Quiet),
        ("Commands\u{2026}", ButtonStyle::Quiet),
    ] {
        view.page.buttons.push(button(label, style));
    }
    view
}

/// Manual test's page of a feature taken over from another machine, its
/// sandbox still running there: Hand back last at the title.
fn feature_tab_manual_hand_back() -> super::FeatureTabView {
    use super::{ButtonStyle, PageButton};
    let mut view = feature_tab_manual_test(false);
    view.page.buttons.push(PageButton {
        label: "Hand back".into(),
        style: ButtonStyle::Plain,
    });
    view
}

/// Manual test's page with its fix rounds: one making its fixes, open, on
/// top; in the history under the criteria one over and shown, and one
/// dropped, folded.
fn feature_tab_manual_rounds() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, Dot, PageButton, PageSection, RoundAsked, RoundCard};
    let mut view = feature_tab_manual_test(false);
    let first = view.page.buttons.len();
    for (label, style) in [
        ("Open", ButtonStyle::Plain),
        ("Diff", ButtonStyle::Plain),
        ("Hide", ButtonStyle::Plain),
        ("Diff", ButtonStyle::Plain),
        ("Open plan", ButtonStyle::Plain),
        ("Hide", ButtonStyle::Plain),
        ("Open plan", ButtonStyle::Plain),
        ("Show", ButtonStyle::Plain),
    ] {
        view.page.buttons.push(PageButton {
            label: label.into(),
            style,
        });
    }
    let asked = |id: &str, title: &str, note: &str, now: Option<(&str, CheckTone)>| RoundAsked {
        id: id.into(),
        title: title.into(),
        note: note.into(),
        now: now.map(|(text, tone)| tab_line(text, tone)),
    };
    let cards = [
        RoundCard {
            label: fix_label(3),
            dot: Dot::Working,
            state: tab_line(
                "manual test failures \u{b7} making fixes \u{b7} 1 of 2 made",
                CheckTone::Note,
            ),
            meta: "asked by Manual test 20m ago \u{b7} made by the Build agent (Claude Code, \
                   opus) \u{b7} fixes went 12m ago"
                .into(),
            asked: vec![
                asked(
                    "AC-2",
                    "Export off with no invoices",
                    "The button stays on and exports an empty file",
                    Some(("being fixed", CheckTone::Note)),
                ),
                asked(
                    "AC-4",
                    "Totals in the footer",
                    "",
                    Some(("not tried since", CheckTone::Note)),
                ),
            ],
            asked_caption: "Asked to fix",
            fixes: vec![
                (true, "Disable Export with no invoices".into()),
                (false, "Footer sums the shown rows".into()),
            ],
            running: vec![1],
            changed: vec!["2 commits \u{b7} 3 files \u{b7} +48 \u{2212}9".into()],
            open: true,
            nested: false,
            buttons: vec![first, first + 1, first + 2],
        },
        RoundCard {
            label: fix_label(2),
            dot: Dot::Done,
            state: tab_line("2 of 2 made", CheckTone::Ready),
            meta: "asked by Manual test 2h ago \u{b7} made by the Build agent (Cursor) \u{b7} \
                   fixes went 2h ago \u{b7} finished 1h ago \u{b7} took 34 min"
                .into(),
            asked: vec![asked(
                "AC-3",
                "CSV keeps the currency",
                "Amounts lose their symbol (2 screenshots)",
                Some(("passed since", CheckTone::Ready)),
            )],
            asked_caption: "Asked to fix",
            fixes: vec![
                (true, "Write the currency column".into()),
                (true, "Test the CSV header".into()),
            ],
            running: Vec::new(),
            changed: vec!["1 commit \u{b7} 2 files \u{b7} +31 \u{2212}4".into()],
            open: true,
            nested: false,
            buttons: vec![first + 3, first + 4, first + 5],
        },
        RoundCard {
            label: fix_label(1),
            dot: Dot::Unknown,
            state: tab_line("dropped", CheckTone::Note),
            meta: "asked by Manual test 1d ago \u{b7} made by the Build agent (Claude Code)".into(),
            asked: Vec::new(),
            asked_caption: "Asked to fix",
            fixes: Vec::new(),
            running: Vec::new(),
            changed: Vec::new(),
            open: false,
            nested: false,
            buttons: vec![first + 6, first + 7],
        },
    ];
    let mut cards = cards.into_iter().map(PageSection::Round);
    view.page
        .sections
        .insert(0, cards.next().expect("the open round"));
    view.page.sections.push(PageSection::Lines {
        label: "History".into(),
        lines: Vec::new(),
    });
    view.page.sections.extend(cards);
    view
}

/// AI review's page as `features::stage_page` lays it out at run 6: round 9
/// open on top, scrolled down to the history of runs 5 to 1, each over the
/// rounds it asked for.
fn feature_tab_ai_review_history() -> super::FeatureTabView {
    let mut view = crate::features::tests::history_view();
    view.scroll[super::TabColumn::Page as usize] = 480.0;
    view
}

/// What fix round `n` is called, as its card says it.
fn fix_label(n: u32) -> String {
    crate::feature::fix_round::label(n)
}

/// Fix round `n`'s card on a fixture's page, open or folded, with no buttons.
fn round_card(
    n: u32,
    dot: super::Dot,
    state: (&str, super::CheckTone),
    meta: &str,
    fixes: &[(bool, &str)],
    open: bool,
) -> super::PageSection {
    super::PageSection::Round(super::RoundCard {
        label: fix_label(n),
        dot,
        state: tab_line(state.0, state.1),
        meta: meta.into(),
        asked: Vec::new(),
        asked_caption: "Asked to fix",
        fixes: fixes.iter().map(|(d, t)| (*d, (*t).to_owned())).collect(),
        running: Vec::new(),
        changed: Vec::new(),
        open,
        nested: false,
        buttons: Vec::new(),
    })
}

/// `view` as the tab gets it: its buttons where `features::place_buttons`
/// and `in_menu` put them, unless the fixture placed them; with Build's
/// footer kept, Enter saying the primary's label (a card's, if it holds it),
/// as the footer says the button, and Tab going to the page's cards.
fn placed(mut view: super::FeatureTabView) -> super::FeatureTabView {
    use super::{ButtonStyle, PageSection};
    if view.page.primary.is_none() {
        // Starting over is a row of `⋯`, as `features::in_menu` has it.
        if view.page.menu.is_empty() {
            view.page.menu = (view.page.buttons.iter().enumerate())
                .filter(|(_, b)| b.label == "Start over\u{2026}")
                .map(|(i, _)| i)
                .collect();
        }
        crate::features::place_buttons(&mut view.page);
    }
    if view.hints == feature_tab().hints {
        // Tab goes to the page's cards first, named as the footer names them.
        let cards = (view.page.sections.iter()).find_map(|s| match s {
            PageSection::ShipGroup(_) => Some("steps"),
            PageSection::Finding(c) if c.sheet.is_some() => Some("criteria"),
            PageSection::Finding(c) if c.tag.dot().is_some() => Some("checks"),
            PageSection::Finding(_) => Some(match view.page.title.as_str() {
                "AI review" => "findings",
                "Plan review" => "suggestions",
                _ => "comments",
            }),
            _ => None,
        });
        if let Some(cards) = cards {
            view.hints[2].1 = cards.to_owned();
        }
        let buttons = &view.page.buttons;
        let primary = (view.page.primary)
            .or_else(|| buttons.iter().position(|b| b.style == ButtonStyle::Primary))
            .and_then(|i| buttons.get(i));
        match primary.map(|b| crate::features::lower_first(&b.label)) {
            Some(said) => view.hints[1].1 = said,
            None => drop(view.hints.remove(1)),
        }
    }
    view
}

/// The feature tab's fixtures, by name.
fn feature_tabs() -> [(&'static str, super::FeatureTabView); 60] {
    let views = [
        ("tab_build", feature_tab()),
        ("tab_verify_fixes", feature_tab_verify_fixes()),
        ("tab_open_round", feature_tab_open_round()),
        ("tab_plan_ready", feature_tab_plan_ready()),
        ("tab_verify_failed", feature_tab_verify_failed()),
        ("tab_verify_running", feature_tab_verify_running()),
        ("tab_fix_plan", feature_tab_fix_plan()),
        ("tab_ship_blocked", feature_tab_ship_blocked()),
        ("tab_ship_routes", feature_tab_ship_routes()),
        ("tab_ship_finish", feature_tab_ship_finish()),
        ("tab_ship_live", feature_tab_ship_live()),
        ("tab_ship_cards", feature_tab_ship_cards()),
        ("tab_ship_conflicts", feature_tab_ship_conflicts()),
        ("tab_ship_ci", feature_tab_ship_ci()),
        ("tab_ship_base_ci", feature_tab_ship_base_ci()),
        ("tab_ship_stopped", feature_tab_ship_stopped()),
        ("tab_ship_after", feature_tab_ship_after()),
        ("tab_done", feature_tab_done()),
        ("tab_later", feature_tab_later()),
        ("tab_review_page", feature_tab_review_page()),
        ("tab_repos", feature_tab_repos()),
        ("tab_unfolded", feature_tab_unfolded()),
        ("tab_shipped", feature_tab_shipped()),
        ("tab_repos_failed", feature_tab_repos_failed()),
        ("tab_repos_error", feature_tab_repos_error()),
        ("tab_more_repos", feature_tab_more_repos()),
        ("tab_more_repos_mid", feature_tab_more_repos_mid()),
        ("tab_rail_wrap", feature_tab_rail_wrap()),
        ("tab_picks_later", feature_tab_picks_later()),
        ("tab_picks_running", feature_tab_picks_running()),
        (
            "tab_model_later",
            with_model_button(feature_tab_picks_later(), "Model: default"),
        ),
        (
            "tab_model_long",
            with_model_button(
                feature_tab_picks_running(),
                "Model: claude-opus-4-8[context=1m,effort=high,fast=false]",
            ),
        ),
        ("tab_t3_draft", feature_tab_t3_draft()),
        ("tab_milestone_mode", feature_tab_milestone_mode()),
        ("tab_held", feature_tab_held()),
        ("tab_resume", feature_tab_resume()),
        ("tab_fixing", feature_tab_fixing()),
        ("tab_fixed_code", feature_tab_fixed_code()),
        ("tab_approved", feature_tab_approved()),
        ("tab_locked", feature_tab_locked()),
        ("tab_start_over", feature_tab_start_over()),
        ("tab_discover_brief", feature_tab_discover_brief()),
        ("tab_discover_decisions", feature_tab_discover_decisions()),
        ("tab_discover_research", feature_tab_discover_research()),
        ("tab_plan_long", feature_tab_plan_long()),
        ("tab_manual_test", feature_tab_manual_test(false)),
        ("tab_manual_note", feature_tab_manual_test(true)),
        ("tab_manual_pages", feature_tab_manual_pages()),
        ("tab_manual_search", feature_tab_manual_search()),
        ("tab_manual_select", feature_tab_manual_select()),
        ("tab_review_rounds", feature_tab_review_rounds()),
        ("tab_manual_rounds", feature_tab_manual_rounds()),
        ("tab_manual_comment", feature_tab_manual_comment()),
        ("tab_manual_hand_back", feature_tab_manual_hand_back()),
        ("tab_ai_review_history", feature_tab_ai_review_history()),
        ("tab_fix_picks", feature_tab_fix_picks()),
        ("tab_ship_picks", feature_tab_ship_picks()),
        ("tab_ship_checking", feature_tab_ship_checking()),
        ("tab_verify_regressed", feature_tab_verify_regressed()),
        ("tab_verify_machine", feature_tab_verify_machine()),
    ];
    views.map(|(name, view)| (name, placed(view)))
}

/// Each fixture at the width it shows best at, and Build at all three.
const TAB_SURFACES: [(&str, i32); 77] = [
    ("tab_build", 860),
    ("tab_build", 1180),
    ("tab_build", 1920),
    ("tab_plan_ready", 1180),
    ("tab_verify_failed", 1920),
    ("tab_verify_running", 1180),
    ("tab_fix_plan", 860),
    ("tab_ship_blocked", 1180),
    ("tab_ship_routes", 1180),
    ("tab_ship_routes", 860),
    ("tab_ship_finish", 1180),
    ("tab_done", 1920),
    ("tab_later", 860),
    ("tab_repos", 1180),
    ("tab_unfolded", 860),
    ("tab_unfolded", 1920),
    ("tab_shipped", 1180),
    ("tab_repos_failed", 1180),
    ("tab_repos_error", 860),
    ("tab_more_repos", 850),
    ("tab_more_repos_mid", 850),
    ("tab_rail_wrap", 500),
    ("tab_rail_wrap", 1180),
    ("tab_picks_later", 1180),
    ("tab_picks_running", 860),
    ("tab_model_later", 1180),
    ("tab_model_long", 860),
    ("tab_t3_draft", 860),
    ("tab_milestone_mode", 1180),
    ("tab_milestone_mode", 860),
    ("tab_held", 860),
    ("tab_resume", 1180),
    ("tab_resume", 860),
    ("tab_fixing", 1180),
    ("tab_fixed_code", 1180),
    ("tab_fixed_code", 860),
    ("tab_approved", 1180),
    ("tab_locked", 1180),
    ("tab_start_over", 1180),
    ("tab_discover_brief", 1180),
    ("tab_discover_brief", 860),
    ("tab_plan_long", 1180),
    ("tab_manual_test", 1180),
    ("tab_manual_note", 860),
    ("tab_review_rounds", 1180),
    ("tab_verify_fixes", 1180),
    ("tab_verify_fixes", 860),
    ("tab_review_page", 1180),
    ("tab_open_round", 1180),
    ("tab_open_round", 860),
    ("tab_manual_pages", 860),
    ("tab_manual_search", 860),
    ("tab_manual_select", 860),
    ("tab_manual_rounds", 1180),
    ("tab_manual_rounds", 860),
    ("tab_manual_comment", 860),
    ("tab_manual_hand_back", 1180),
    ("tab_ship_live", 1180),
    ("tab_ship_live", 860),
    ("tab_ship_cards", 1180),
    ("tab_ship_conflicts", 860),
    ("tab_ship_after", 1180),
    ("tab_ai_review_history", 1920),
    ("tab_ai_review_history", 860),
    ("tab_fix_picks", 1180),
    ("tab_fix_picks", 860),
    ("tab_ship_picks", 1180),
    ("tab_ship_picks", 860),
    ("tab_ship_ci", 860),
    ("tab_ship_base_ci", 1180),
    ("tab_ship_stopped", 860),
    ("tab_ship_checking", 1180),
    ("tab_verify_regressed", 1180),
    ("tab_verify_machine", 860),
    ("tab_discover_decisions", 860),
    ("tab_discover_research", 1180),
    ("tab_discover_research", 860),
];

const FEATURE_TAB: [&str; 77] = [
    "tab_build_860",
    "tab_build_1180",
    "tab_build_1920",
    "tab_plan_ready_1180",
    "tab_verify_failed_1920",
    "tab_verify_running_1180",
    "tab_fix_plan_860",
    "tab_ship_blocked_1180",
    "tab_ship_routes_1180",
    "tab_ship_routes_860",
    "tab_ship_finish_1180",
    "tab_done_1920",
    "tab_later_860",
    "tab_repos_1180",
    "tab_unfolded_860",
    "tab_unfolded_1920",
    "tab_shipped_1180",
    "tab_repos_failed_1180",
    "tab_repos_error_860",
    "tab_more_repos_850",
    "tab_more_repos_mid_850",
    "tab_rail_wrap_500",
    "tab_rail_wrap_1180",
    "tab_picks_later_1180",
    "tab_picks_running_860",
    "tab_model_later_1180",
    "tab_model_long_860",
    "tab_t3_draft_860",
    "tab_milestone_mode_1180",
    "tab_milestone_mode_860",
    "tab_held_860",
    "tab_resume_1180",
    "tab_resume_860",
    "tab_fixing_1180",
    "tab_fixed_code_1180",
    "tab_fixed_code_860",
    "tab_approved_1180",
    "tab_locked_1180",
    "tab_start_over_1180",
    "tab_discover_brief_1180",
    "tab_discover_brief_860",
    "tab_plan_long_1180",
    "tab_manual_test_1180",
    "tab_manual_note_860",
    "tab_review_rounds_1180",
    "tab_verify_fixes_1180",
    "tab_verify_fixes_860",
    "tab_review_page_1180",
    "tab_open_round_1180",
    "tab_open_round_860",
    "tab_manual_pages_860",
    "tab_manual_search_860",
    "tab_manual_select_860",
    "tab_manual_rounds_1180",
    "tab_manual_rounds_860",
    "tab_manual_comment_860",
    "tab_manual_hand_back_1180",
    "tab_ship_live_1180",
    "tab_ship_live_860",
    "tab_ship_cards_1180",
    "tab_ship_conflicts_860",
    "tab_ship_after_1180",
    "tab_ai_review_history_1920",
    "tab_ai_review_history_860",
    "tab_fix_picks_1180",
    "tab_fix_picks_860",
    "tab_ship_picks_1180",
    "tab_ship_picks_860",
    "tab_ship_ci_860",
    "tab_ship_base_ci_1180",
    "tab_ship_stopped_860",
    "tab_ship_checking_1180",
    "tab_verify_regressed_1180",
    "tab_verify_machine_860",
    "tab_discover_decisions_860",
    "tab_discover_research_1180",
    "tab_discover_research_860",
];

/// The recorded hashes of `FEATURE_TAB`, per case in `CASES`.
const GOLDEN_FEATURE_TAB: [[u64; 77]; 4] = [
    // Dark at 1
    [
        0xa85bec63adf6c004, // tab_build_860
        0x91cba3eb57f20bc5, // tab_build_1180
        0x6c6af14a5c13c653, // tab_build_1920
        0x77c0a93f9a9a40d8, // tab_plan_ready_1180
        0x2752b7467da8e51a, // tab_verify_failed_1920
        0xf993d0a2e3b92464, // tab_verify_running_1180
        0x181e5b0cc4efad20, // tab_fix_plan_860
        0x545a4e8f31519be1, // tab_ship_blocked_1180
        0xcc6b63362b0cdd20, // tab_ship_routes_1180
        0x372d8bf5b1d2b611, // tab_ship_routes_860
        0xbc3bfdc2b2957b04, // tab_ship_finish_1180
        0x246c2ab154996137, // tab_done_1920
        0x33f260e2d2b73662, // tab_later_860
        0x8922c10bd9859e96, // tab_repos_1180
        0xbbee296b91c0a971, // tab_unfolded_860
        0x4342b7c70df05012, // tab_unfolded_1920
        0x72c5cf5ddbd6eed8, // tab_shipped_1180
        0x6112452c774a71a7, // tab_repos_failed_1180
        0x2325428e44ebd5f1, // tab_repos_error_860
        0x4a12f770a14aa9a8, // tab_more_repos_850
        0x5593c071206e1769, // tab_more_repos_mid_850
        0x28ef8163f09872c3, // tab_rail_wrap_500
        0xf6574568f3f2bda9, // tab_rail_wrap_1180
        0xa1785204f63f8385, // tab_picks_later_1180
        0x53dead8f6dc0c522, // tab_picks_running_860
        0x3f6bb9eaf873caba, // tab_model_later_1180
        0xc0ebb3657f85499e, // tab_model_long_860
        0x64897ab87ba9549e, // tab_t3_draft_860
        0x1898b8544317bfbc, // tab_milestone_mode_1180
        0x8debc81b08683f41, // tab_milestone_mode_860
        0x7d2acc0bca0b9006, // tab_held_860
        0x210979fd58f5c634, // tab_resume_1180
        0x5eb019af39610302, // tab_resume_860
        0xfe631416ff8a726a, // tab_fixing_1180
        0x4557b19df663481c, // tab_fixed_code_1180
        0x75d914171e4d41b1, // tab_fixed_code_860
        0x3d434dc9db1e215e, // tab_approved_1180
        0xe3871de3d587650e, // tab_locked_1180
        0xde1375bb0f72b838, // tab_start_over_1180
        0xc98f3fca337e3cec, // tab_discover_brief_1180
        0x00b77ce320ec7440, // tab_discover_brief_860
        0x2d81ce586ef2e95c, // tab_plan_long_1180
        0x0b214bda6b3e0445, // tab_manual_test_1180
        0xfeffbb22941e4803, // tab_manual_note_860
        0x845eeaaddaa14ba2, // tab_review_rounds_1180
        0x447385b7f0cd86b3, // tab_verify_fixes_1180
        0xd3c156b8d53e8586, // tab_verify_fixes_860
        0x8e71dc34206afb28, // tab_review_page_1180
        0xea5bfe12d5c3c305, // tab_open_round_1180
        0x3d4c4e32d9693834, // tab_open_round_860
        0x2635761d6794e571, // tab_manual_pages_860
        0xaf24f2ae2380d731, // tab_manual_search_860
        0x8a920092593bec6c, // tab_manual_select_860
        0x611bd3eb9d05f64a, // tab_manual_rounds_1180
        0x9bfd8ce56a1f147b, // tab_manual_rounds_860
        0x3205d5c5248e0cd8, // tab_manual_comment_860
        0xabaa4eb37b3cb65d, // tab_manual_hand_back_1180
        0x511df75a3a413b77, // tab_ship_live_1180
        0xcf84840da9cfdb67, // tab_ship_live_860
        0x88c86117365f7b0e, // tab_ship_cards_1180
        0xc819cc2c78fe6cb3, // tab_ship_conflicts_860
        0x540748a31dc2da68, // tab_ship_after_1180
        0xa5868663bc4871e9, // tab_ai_review_history_1920
        0x939f12fe8fea647f, // tab_ai_review_history_860
        0x79d920ee48fd1aea, // tab_fix_picks_1180
        0x8aec8f83bf70883c, // tab_fix_picks_860
        0x61e7db0fcb63dcec, // tab_ship_picks_1180
        0xf40ac477d14785e5, // tab_ship_picks_860
        0x85ff726fc5f0a98d, // tab_ship_ci_860
        0xd0d7623f25a0db15, // tab_ship_base_ci_1180
        0x5abc2fc5ea8d0aca, // tab_ship_stopped_860
        0x0d29fb85dd2ec61f, // tab_ship_checking_1180
        0xd79f2ae07b504dcb, // tab_verify_regressed_1180
        0x4b2e2423c7e42e6c, // tab_verify_machine_860
        0xda70f276530966ba, // tab_discover_decisions_860
        0xa7707c8af4d7f72f, // tab_discover_research_1180
        0x9c584d88ce6ba23e, // tab_discover_research_860
    ],
    // Dark at 2
    [
        0x07d3629491140f00, // tab_build_860
        0xa57a530bdc6e6e0e, // tab_build_1180
        0x9a6e49338cdec30c, // tab_build_1920
        0x722a9acbcc31b194, // tab_plan_ready_1180
        0x0a4fc53c9ab45583, // tab_verify_failed_1920
        0x43eb6027dca6440a, // tab_verify_running_1180
        0xa6b52546771223cf, // tab_fix_plan_860
        0xf96a3da25467fcbe, // tab_ship_blocked_1180
        0xb1e9f0a81ba0e276, // tab_ship_routes_1180
        0x8edfe2586b1ccc70, // tab_ship_routes_860
        0x3ac06dfad8f991b9, // tab_ship_finish_1180
        0x203cd3c34861467e, // tab_done_1920
        0xe72e175cf7fe120e, // tab_later_860
        0x8cb89fc2fb496e15, // tab_repos_1180
        0xd4eb7aa1dfd0f0cd, // tab_unfolded_860
        0x3bc82e5af052255d, // tab_unfolded_1920
        0x18a437e7f57b2254, // tab_shipped_1180
        0x12cd3299c2b6951b, // tab_repos_failed_1180
        0xc5eb59296cc7aecf, // tab_repos_error_860
        0x929c182498647d0b, // tab_more_repos_850
        0x76fbf04047009dfc, // tab_more_repos_mid_850
        0xdef9b895869055f2, // tab_rail_wrap_500
        0x0b3d0e257741e089, // tab_rail_wrap_1180
        0xfc58e6de433d0405, // tab_picks_later_1180
        0xebaed4839e1a6b54, // tab_picks_running_860
        0xfb4bd096dfb10963, // tab_model_later_1180
        0x840a43f732f21705, // tab_model_long_860
        0xd334f28552de95e6, // tab_t3_draft_860
        0xc53297e40c183eb3, // tab_milestone_mode_1180
        0xdc715ca1649c9769, // tab_milestone_mode_860
        0x1ba1af4eeca251ad, // tab_held_860
        0x0b525c96ff49da3c, // tab_resume_1180
        0x742e5a01c600e4bd, // tab_resume_860
        0xc5af43845efbbd0a, // tab_fixing_1180
        0x11c447d3ca9faf40, // tab_fixed_code_1180
        0xcbf66f0aa859084e, // tab_fixed_code_860
        0x4bcbc042005c754d, // tab_approved_1180
        0x847bc973d1e2c319, // tab_locked_1180
        0x01e74efc6dd663f5, // tab_start_over_1180
        0x39a2633c2312f00c, // tab_discover_brief_1180
        0x4922b110863e774c, // tab_discover_brief_860
        0xa9dc4098dc5a6e57, // tab_plan_long_1180
        0x0e23b2d60cac83bb, // tab_manual_test_1180
        0x84f84a785822fb5f, // tab_manual_note_860
        0xe95dc57e85689675, // tab_review_rounds_1180
        0x82023e42d1098697, // tab_verify_fixes_1180
        0x1e84c566da94bbb7, // tab_verify_fixes_860
        0x8410c94efe103491, // tab_review_page_1180
        0xb485b5ee7673cda9, // tab_open_round_1180
        0x10cdc3419dade9db, // tab_open_round_860
        0x0e2c01b77cd783a9, // tab_manual_pages_860
        0xcf66932df97aa1aa, // tab_manual_search_860
        0xf7368ac002e96423, // tab_manual_select_860
        0x1d1719f43c1c2b8b, // tab_manual_rounds_1180
        0x6020c7eccd5a1711, // tab_manual_rounds_860
        0x1029a022a337b8de, // tab_manual_comment_860
        0x8a9cd470c6a0a122, // tab_manual_hand_back_1180
        0xea59ac9925f87be4, // tab_ship_live_1180
        0xe6283ff4242dc5d7, // tab_ship_live_860
        0xd130d378b5e235e5, // tab_ship_cards_1180
        0xa5f85edeba7aa17d, // tab_ship_conflicts_860
        0xea4308df5eb3d2ef, // tab_ship_after_1180
        0x3329732fafe9ad49, // tab_ai_review_history_1920
        0xdb1be6c9d37e5120, // tab_ai_review_history_860
        0x4208f92c64f08639, // tab_fix_picks_1180
        0x3b08557cfdb4d766, // tab_fix_picks_860
        0x3b87a841331d3640, // tab_ship_picks_1180
        0xe95a027c61b41224, // tab_ship_picks_860
        0x63d540992bb3c807, // tab_ship_ci_860
        0xc19d62726306ca17, // tab_ship_base_ci_1180
        0xe42047e5325b42f5, // tab_ship_stopped_860
        0x7bf7bd484bd1418c, // tab_ship_checking_1180
        0xb57df7a155554e8c, // tab_verify_regressed_1180
        0x6be6e8dccf97d26e, // tab_verify_machine_860
        0x32215b41acad6c0f, // tab_discover_decisions_860
        0x0105900a76d466c7, // tab_discover_research_1180
        0x4af5931c481e0384, // tab_discover_research_860
    ],
    // Light at 1
    [
        0x766d41996c532433, // tab_build_860
        0x90db1ab58555ff26, // tab_build_1180
        0x0e14cd53fb2d2b5c, // tab_build_1920
        0xb413241c3e523209, // tab_plan_ready_1180
        0x394ead563935030a, // tab_verify_failed_1920
        0x0f4fce1e32dd6c6d, // tab_verify_running_1180
        0xd0aee6c4fe03195b, // tab_fix_plan_860
        0xb2f52275c86ee89a, // tab_ship_blocked_1180
        0xde79fa95987b78fb, // tab_ship_routes_1180
        0xd36f541f3b2e9fda, // tab_ship_routes_860
        0x7da843de3b6a83f3, // tab_ship_finish_1180
        0x3a8990383b765f33, // tab_done_1920
        0x2dd4b2fa567ab130, // tab_later_860
        0x58d6634d3d5aefbc, // tab_repos_1180
        0xab116ed6f1c71601, // tab_unfolded_860
        0x786603ba33c8e5ba, // tab_unfolded_1920
        0x7ddfa77e81a2daff, // tab_shipped_1180
        0x47602148c0a1bfd4, // tab_repos_failed_1180
        0x9a804ac7bbe0a52f, // tab_repos_error_860
        0x0446d80dd0b70370, // tab_more_repos_850
        0x05f71371a3cc30a5, // tab_more_repos_mid_850
        0x7b28837bd2d0891d, // tab_rail_wrap_500
        0xa97498e34431663f, // tab_rail_wrap_1180
        0x295529f3ebe348c6, // tab_picks_later_1180
        0x9ee08d065c407d7a, // tab_picks_running_860
        0xf535e318b743105b, // tab_model_later_1180
        0xc1683c01713422b2, // tab_model_long_860
        0xa4a8a96de0e7ca65, // tab_t3_draft_860
        0xaf915c750571698f, // tab_milestone_mode_1180
        0xf99211ee19503732, // tab_milestone_mode_860
        0xd474de592c1955d4, // tab_held_860
        0x641afd71c7a4d716, // tab_resume_1180
        0x554d0fe73a73a81e, // tab_resume_860
        0xabe0576b6b74ca5d, // tab_fixing_1180
        0x1e5cfaad2f0deae0, // tab_fixed_code_1180
        0x815a0155066889a5, // tab_fixed_code_860
        0x0cefd23a75d57aa9, // tab_approved_1180
        0x7fa92069c4c6c7c4, // tab_locked_1180
        0x3b9489692fe604d9, // tab_start_over_1180
        0xd3eb1b44fc74f034, // tab_discover_brief_1180
        0x257eb40f1cff75dc, // tab_discover_brief_860
        0xa9c0a65645710b35, // tab_plan_long_1180
        0x1b64f6d06930bdb7, // tab_manual_test_1180
        0xd9031b2f93e41891, // tab_manual_note_860
        0xb9053095cee7e231, // tab_review_rounds_1180
        0xff7066fd4ef3652e, // tab_verify_fixes_1180
        0xfe2f3ff58b9c06bb, // tab_verify_fixes_860
        0x7f34709ee2573d89, // tab_review_page_1180
        0x252c4149f706bb3c, // tab_open_round_1180
        0x222653714ba092d1, // tab_open_round_860
        0xc51e68483a2a2f24, // tab_manual_pages_860
        0x1062750767fde6f5, // tab_manual_search_860
        0x1de5c09af720ce84, // tab_manual_select_860
        0x7f83f75e2eef809e, // tab_manual_rounds_1180
        0xaff4cc7a50a5dab7, // tab_manual_rounds_860
        0x1d652ad6c6c42b20, // tab_manual_comment_860
        0x79652b049c011ef3, // tab_manual_hand_back_1180
        0x8a3b61ac8bc3022f, // tab_ship_live_1180
        0x15b816f593149cee, // tab_ship_live_860
        0xda6598725db719d1, // tab_ship_cards_1180
        0xf0074b4f71eaf798, // tab_ship_conflicts_860
        0xa7426ac32ad331cb, // tab_ship_after_1180
        0xa4de3151bd481b40, // tab_ai_review_history_1920
        0x66b585fc4ecec124, // tab_ai_review_history_860
        0xfc27f9b4fad020c8, // tab_fix_picks_1180
        0x157b21d591211027, // tab_fix_picks_860
        0x3f7cd5fc84f04aae, // tab_ship_picks_1180
        0xa3f5a4917bc74b18, // tab_ship_picks_860
        0xe8f404b65e5a2d6a, // tab_ship_ci_860
        0x9d8df21bd6463d35, // tab_ship_base_ci_1180
        0xdcd837b221117ba0, // tab_ship_stopped_860
        0xb095820b01c1f30f, // tab_ship_checking_1180
        0xfcff387c15772795, // tab_verify_regressed_1180
        0x5d49ce354f5d4282, // tab_verify_machine_860
        0x60d42930304868fe, // tab_discover_decisions_860
        0x5827aad08a39b0df, // tab_discover_research_1180
        0xcb16b4b276f98a61, // tab_discover_research_860
    ],
    // Light at 2
    [
        0xcd90db3361ed5e99, // tab_build_860
        0x48dce04797ca6d3e, // tab_build_1180
        0x7a9632591d9b0514, // tab_build_1920
        0x0f3d89f973e99881, // tab_plan_ready_1180
        0x3f8fd57491c0e0fd, // tab_verify_failed_1920
        0x1d25c46e6dd339c6, // tab_verify_running_1180
        0x5643b25dc7b39750, // tab_fix_plan_860
        0x8663da01db073316, // tab_ship_blocked_1180
        0x1ac933cbb89497db, // tab_ship_routes_1180
        0x5b12feb7cc7a47a0, // tab_ship_routes_860
        0x284015889fa794e8, // tab_ship_finish_1180
        0x5ca66d192a7bc6d6, // tab_done_1920
        0x8b6774af4d2e9073, // tab_later_860
        0xc2834ea27255e775, // tab_repos_1180
        0x02494e40a5258c4e, // tab_unfolded_860
        0xd4e3d0b711c4a9f3, // tab_unfolded_1920
        0x7c3e68dec49dfc6e, // tab_shipped_1180
        0xc6497df95e809460, // tab_repos_failed_1180
        0x0966304a6ff5818f, // tab_repos_error_860
        0xcc1c0e46511ba4d0, // tab_more_repos_850
        0x0e31fdc4c0cbb580, // tab_more_repos_mid_850
        0x4850294a01b29e07, // tab_rail_wrap_500
        0x7f89a0f63cba2c24, // tab_rail_wrap_1180
        0x88185f506c608c2c, // tab_picks_later_1180
        0xb61f249d3fdf9ed4, // tab_picks_running_860
        0x558386a6e1a53356, // tab_model_later_1180
        0xfcbca7511da7d55f, // tab_model_long_860
        0xcce4c7c6e2bd6253, // tab_t3_draft_860
        0xbe12144dace45346, // tab_milestone_mode_1180
        0xbf432d672e9beee5, // tab_milestone_mode_860
        0x196a4fe48a3ca7c2, // tab_held_860
        0xe18ba0f05bc026be, // tab_resume_1180
        0xddd99cdc03b33e53, // tab_resume_860
        0xe3d9cba5348d3167, // tab_fixing_1180
        0x477e5a5bec46210a, // tab_fixed_code_1180
        0xdf43b26432f54d2d, // tab_fixed_code_860
        0x2f60b16c10d0cf4e, // tab_approved_1180
        0xd2359e9cd1cafff7, // tab_locked_1180
        0x82d17e167ef6a9a1, // tab_start_over_1180
        0x736893c680a2923c, // tab_discover_brief_1180
        0x18caf63ace3ca05b, // tab_discover_brief_860
        0xec428563fd7b1234, // tab_plan_long_1180
        0x8394e4a5692e0044, // tab_manual_test_1180
        0x5778ee0ab01e5928, // tab_manual_note_860
        0xe6c794c1d80467ba, // tab_review_rounds_1180
        0x199cd631fc026353, // tab_verify_fixes_1180
        0x4fa7eb9fa08d15ac, // tab_verify_fixes_860
        0x526b52c2624a2972, // tab_review_page_1180
        0x7162fc7471c6ca62, // tab_open_round_1180
        0x64fdbb92163df805, // tab_open_round_860
        0x2ab565b9f6f8b65d, // tab_manual_pages_860
        0x70874258ea2ece90, // tab_manual_search_860
        0xeb2b65721af65fec, // tab_manual_select_860
        0x61c94277adfe73e7, // tab_manual_rounds_1180
        0x981d864d7a9c7f3c, // tab_manual_rounds_860
        0x366055942599ade1, // tab_manual_comment_860
        0xff541106e7c4e830, // tab_manual_hand_back_1180
        0xd8367872731903b9, // tab_ship_live_1180
        0x2ab13b28404068e4, // tab_ship_live_860
        0x6cbd33c9296883a3, // tab_ship_cards_1180
        0x27a5f72695ef10aa, // tab_ship_conflicts_860
        0xa8f5e6f731b3c642, // tab_ship_after_1180
        0x0eedc23bf52cf115, // tab_ai_review_history_1920
        0xbd19694b6fdefd08, // tab_ai_review_history_860
        0x570f6f5da18dc4ee, // tab_fix_picks_1180
        0x56002719ecef3c98, // tab_fix_picks_860
        0xb31dfc4e4b93e5cb, // tab_ship_picks_1180
        0x0190d7fa623a858c, // tab_ship_picks_860
        0x7b8deae3f8385f48, // tab_ship_ci_860
        0x72eb82655e5c874a, // tab_ship_base_ci_1180
        0x14504ee37fc766d1, // tab_ship_stopped_860
        0xfb149b0388901b80, // tab_ship_checking_1180
        0x3ffeb089fc4e2b3e, // tab_verify_regressed_1180
        0x42271a11cd38aa76, // tab_verify_machine_860
        0x8c54bef25df12f97, // tab_discover_decisions_860
        0x6ca795c6200c934b, // tab_discover_research_1180
        0x20ae7568bffcea0d, // tab_discover_research_860
    ],
];

#[test]
fn the_feature_tab_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_FEATURE_TAB
        .iter()
        .map(|row| row.as_slice())
        .collect();
    let views = feature_tabs();
    check_each(&FEATURE_TAB, &golden, |painter, theme, scale, surface| {
        let (name, w) = TAB_SURFACES[surface];
        let view = &views.iter().find(|(n, _)| *n == name).expect("a fixture").1;
        hash(super::feature_tab_composed(
            painter,
            view,
            (w, TAB_HEIGHT).into(),
            theme,
            scale,
        ))
    });
}

/// The page column drawn in bands holds the pixels it would drawn whole, at
/// fractional scales too, where a band's edge off a whole pixel would show.
#[test]
fn the_page_bands_paint_the_column_as_a_whole() {
    let painter = Painter::new();
    let mut banded = 0;
    for (name, view) in feature_tabs() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for w in [860, 1920] {
                match super::bands_match_column(&painter, &view, (w, TAB_HEIGHT).into(), scale) {
                    Ok(n) => banded += n,
                    Err(why) => panic!("{name} at {w} wide, scale {scale}: {why}"),
                }
            }
        }
    }
    assert!(banded > 0);
}

/// With `GERDR_DUMP_DIR` set, write every feature tab fixture there at
/// each of `TAB_WIDTHS`.
#[test]
fn dump_feature_tab() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    // `GERDR_DUMP_ONLY=tab_discover` writes only the scenes named so: all
    // of them take minutes and gigabytes.
    let only = std::env::var("GERDR_DUMP_ONLY").ok();
    for (theme, scale) in CASES {
        let views = feature_tabs().into_iter();
        for (name, view) in views.filter(|(n, _)| only.as_deref().is_none_or(|o| n.starts_with(o)))
        {
            for w in TAB_WIDTHS {
                dump(
                    &dir,
                    &format!("{name}_{w}_{}_{}", theme.name(), scale),
                    super::feature_tab_composed(
                        &painter,
                        &view,
                        (w, TAB_HEIGHT).into(),
                        theme,
                        scale,
                    ),
                    (w as f64 * scale).round() as usize,
                );
            }
        }
    }
}

/// With `GERDR_DUMP_DIR` set, write the pages with a history there, wide
/// and narrow.
#[test]
fn dump_history() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    let names = [
        "tab_ai_review_history",
        "tab_review_rounds",
        "tab_manual_rounds",
    ];
    for (theme, scale) in CASES.into_iter().filter(|(_, scale)| *scale == 1.0) {
        for (name, view) in feature_tabs()
            .into_iter()
            .filter(|(n, _)| names.contains(n))
        {
            for w in [1920, 860] {
                let tab = super::feature_tab_composed(
                    &painter,
                    &view,
                    (w, TAB_HEIGHT).into(),
                    theme,
                    scale,
                );
                dump(
                    &dir,
                    &format!("{name}_{w}_{}", theme.name()),
                    tab,
                    w as usize,
                );
            }
        }
    }
}

// ---------------------------------------------------- AI review's findings

/// A review page's summary, then `cards`, as
/// `features::review_page` lays them out; `want_all` its button.
fn review_sections(
    page: &mut super::StagePage,
    label: &str,
    (decided, total): (usize, usize),
    words: &str,
    notes: Vec<super::CheckLine>,
    want_all: Option<&str>,
    cards: Vec<super::FindingCard>,
) {
    use super::{ButtonStyle, PageButton, PageSection, ReviewSummary};
    let mut counts: Vec<(super::FindingTag, usize)> = Vec::new();
    for card in &cards {
        match counts.last_mut() {
            Some((tag, n)) if *tag == card.tag => *n += 1,
            _ => counts.push((card.tag, 1)),
        }
    }
    // The page's own button, held by the summary: one already there moves.
    let buttons = want_all
        .map(
            |label| match page.buttons.iter().position(|b| b.label == label) {
                Some(i) => i,
                None => {
                    page.buttons.push(PageButton {
                        label: label.into(),
                        style: ButtonStyle::Plain,
                    });
                    page.buttons.len() - 1
                }
            },
        )
        .into_iter()
        .collect();
    page.sections.push(PageSection::Review(ReviewSummary {
        label: label.into(),
        counts,
        open: Vec::new(),
        decided,
        refused: 0,
        total,
        words: words.into(),
        notes,
        buttons,
    }));
    for mut card in cards {
        // As `features::review_page` has them: each folds, with no heading.
        card.folds = true;
        page.sections.push(PageSection::Finding(card));
    }
}

/// The feature tab on AI review with a card of each kind, in the page's order:
/// the user's own, a finding to decide, one dismissed and closed, and one to
/// fix with the keyboard.
fn tab_findings() -> super::FeatureTabView {
    use crate::feature::review_import::{Finding, FindingState, Severity};
    use crate::review_cards::{CardPage, Item, item_buttons, item_card};
    let mut view = feature_tab();
    view.focus = super::TabArea::Findings;
    view.page.title = "AI review".into();
    view.page.about = "Another model reviews the diff".into();
    view.page.buttons.clear();
    view.page.sections.clear();
    let finding = |id: &str, severity, state, title: &str, line| Finding {
        id: id.into(),
        severity,
        file: "login.py".into(),
        line: Some(line),
        title: title.into(),
        state,
        run: 1,
        placed: true,
        ..Finding::default()
    };
    // The user's own, one to decide, one dismissed with a comment, and one
    // to fix with the user's fix and comment.
    let findings = [
        Finding {
            yours: true,
            file: String::new(),
            line: None,
            body: "It reads badly.".into(),
            ..finding("u1", Severity::Medium, FindingState::Fix, "Rename retry", 0)
        },
        Finding {
            body: "The loop runs range(n - 1), so the last try never happens, and a caller \
                   asking for one retry gets none at all."
                .into(),
            fix: "Stop at n.".into(),
            ..finding(
                "r1-1",
                Severity::High,
                FindingState::Open,
                "Retry stops one short",
                6,
            )
        },
        Finding {
            reason: "gone in the next branch".into(),
            note: "The branch after this one drops the old form.".into(),
            ..finding(
                "r1-3",
                Severity::High,
                FindingState::Dismissed,
                "Unused import left behind by the last refactor of the login page",
                1,
            )
        },
        Finding {
            body: "A user with no name raises AttributeError.".into(),
            my_fix: Some("Fall back to the plain title".into()),
            note: "Mind the tests, and the greeting when the name is only spaces: it should \
                   fall back too."
                .into(),
            ..finding(
                "r1-2",
                Severity::Medium,
                FindingState::Fix,
                "Title breaks with no name",
                2,
            )
        },
    ];
    // Each card as the page makes it, its buttons among the page's.
    let mut made = Vec::new();
    for (i, f) in findings.iter().enumerate() {
        let item = Item::Finding(f);
        let on = CardPage {
            decides: true,
            posts: false,
        };
        let actions = item_buttons(item, on);
        let mut card = item_card(item, false, &actions, false);
        let at = view.page.buttons.len();
        (card.buttons.iter_mut().chain(&mut card.comment_buttons)).for_each(|b| *b += at);
        view.page
            .buttons
            .extend(actions.into_iter().map(|(b, _)| b));
        // The last with the keyboard, opened.
        card.selected = i == 3;
        card.collapsed &= !card.selected;
        made.push(card);
    }
    review_sections(
        &mut view.page,
        "Findings",
        (3, 4),
        "1 to decide \u{b7} 2 to fix \u{b7} 1 dismissed",
        Vec::new(),
        Some("Fix all open"),
        made,
    );
    view
}

/// The feature tab on Verify with a card of each way a check can stand, the
/// second with the keyboard.
fn tab_checks() -> super::FeatureTabView {
    use super::{ButtonStyle, FindingCard, FindingTag, NodeMark, PageButton, PageSection};
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 4, NodeMark::Current, "running");
    view.focus = super::TabArea::Findings;
    view.page.title = "Verify".into();
    view.page.about = Stage::Verify.about().into();
    view.page.status = None;
    view.page.buttons.clear();
    view.page.sections = vec![PageSection::Lines {
        label: "Checks \u{b7} 1 passed \u{b7} 1 failed \u{b7} 1 running \u{b7} 1 stopped \u{b7} 1 not run \u{b7} 1 skipped"
            .into(),
        lines: Vec::new(),
    }];
    let button = |label: &str, style| PageButton {
        label: label.into(),
        style,
    };
    view.page
        .buttons
        .push(button("Run checks", ButtonStyle::Primary));
    view.page
        .buttons
        .push(button("Stop all", ButtonStyle::Quiet));
    type Card<'a> = (
        &'a [&'a str],
        FindingTag,
        &'a str,
        &'a str,
        &'a str,
        &'a [&'a str],
    );
    let cards: [Card; 6] = [
        (
            &["Show", "Run again", "Skip"],
            FindingTag::Passed,
            "cargo test",
            "1m 12s \u{b7} on older code",
            "api \u{b7} 142 passed",
            &[
                "test tree::walks ... ok",
                "test tree::folds ... ok",
                "test result: ok. 142 passed; 0 failed; 0 ignored",
            ],
        ),
        (
            &["Show", "Run again"],
            FindingTag::Failed,
            "npm test",
            "8s",
            "web \u{b7} exited with 1",
            &[
                "FAIL src/Dialog.test.tsx",
                "  \u{25cf} Dialog \u{203a} closes on Esc",
                "    expect(received).toBe(expected) // a very long line of output that has to stop at the edge of its card, whatever the width of the page is",
                "    Expected: false",
            ],
        ),
        (
            &["Show", "Stop"],
            FindingTag::Running,
            "npm run lint",
            "12s \u{b7} took 1m 40s last run",
            "web",
            &["checking src/App.tsx", "", "", ""],
        ),
        (
            &["Show", "Run again"],
            FindingTag::Stopped,
            "./e2e.sh",
            "4s",
            "feature folder \u{b7} stopped",
            &["starting browsers"],
        ),
        (
            &["Skip"],
            FindingTag::NotRun,
            "cargo clippy",
            "",
            "api",
            &[],
        ),
        (
            &["Don't skip"],
            FindingTag::Skipped,
            "cargo bench",
            "",
            "api \u{b7} skipped for this feature",
            &[],
        ),
    ];
    for (i, (labels, tag, title, state, place, preview)) in cards.into_iter().enumerate() {
        let at = view.page.buttons.len();
        for label in labels {
            view.page.buttons.push(button(label, ButtonStyle::Plain));
        }
        view.page.sections.push(PageSection::Finding(FindingCard {
            sheet: None,
            tag,
            title: title.into(),
            state: state.into(),
            place: place.into(),
            body: String::new(),
            preview: preview.iter().map(|line| (*line).to_owned()).collect(),
            fix: String::new(),
            note: String::new(),
            comment: String::new(),
            comment_buttons: Vec::new(),
            head: 0,
            made: false,
            faded: tag == FindingTag::Skipped,
            collapsed: false,
            folds: false,
            stripe: None,
            field: None,
            field_hint: "",
            clock: String::new(),
            selected: i == 1,
            buttons: (at..at + labels.len()).collect(),
        }));
    }
    view
}

/// The finding dialog, editing a finding's fix.
fn finding_dialog(error: bool) -> super::FindingDialogView {
    let field = |label, text: &str, placeholder, rows| {
        let at = text.len();
        super::FindingFieldView {
            label,
            line: crate::text_field::Line {
                text: text.to_owned(),
                caret: at,
                anchor: at,
                scroll: 0,
            },
            placeholder,
            rows,
        }
    };
    super::FindingDialogView {
        title: format!(
            "{} r1-2",
            crate::feature::review_items::words(crate::feature::review_items::ItemOf::Finding)
                .edit_title
        ),
        about: "Title breaks with no name".into(),
        fields: vec![field(
            "Fix",
            "Fall back to the plain title when the user has no name, and add a test for it so \
                 this can't come back later.\nKeep the greeting otherwise.",
            "What should change",
            6,
        )],
        focus: 0,
        action: "Save and fix",
        cancel: "Cancel",
        error: error.then(|| "Say what to fix.".to_owned()),
        hover: None,
        keys: vec![
            ("Tab", "next"),
            ("Shift+Enter", "new line"),
            ("Esc", "close"),
        ],
        shot: None,
    }
}

const FINDINGS: [&str; 6] = [
    "tab_findings_1180",
    "tab_findings_860",
    "finding_dialog",
    "finding_dialog_error",
    "comment_dialog_shot",
    "comment_dialog_pick",
];

/// The dialog adding a comment from the criteria strip, its screenshot
/// checkbox ticked.
fn comment_dialog_shot() -> super::FindingDialogView {
    let mut view = finding_dialog(false);
    view.title = crate::feature::review_items::words(crate::feature::review_items::ItemOf::Finding)
        .add_title
        .to_owned();
    view.about =
        "It goes to the Build agent with the failures, and stays open until you mark it done"
            .into();
    view.fields[0].label = "Comment";
    view.action = "Save";
    view.cancel = "Discard";
    view.shot = Some(true);
    view.keys = vec![("Shift+Enter", "new line"), ("Esc", "close")];
    view
}

/// The dialog adding a comment on an area selected on the app's page: the
/// line saying where under what it is for, no screenshot checkbox, and a
/// draft to keep though nothing is typed.
fn comment_dialog_pick() -> super::FindingDialogView {
    let mut view = comment_dialog_shot();
    view.about = format!(
        "{}\nOn /orders: OrderTable and 2 more \u{b7} screenshot of the area",
        view.about
    );
    view.fields[0].line = Line::default();
    view.shot = None;
    view.keys = vec![
        ("Shift+Enter", "new line"),
        (crate::text_field::DISCARD_KEY, "discard"),
        ("Esc", "keep draft"),
    ];
    view
}

/// The recorded hashes of `FINDINGS`, per case in `CASES`.
const GOLDEN_FINDINGS: [[u64; 6]; 4] = [
    // Dark at 1
    [
        0x84b54b46a4c2057c, // tab_findings_1180
        0xf64f9689b9ff0c87, // tab_findings_860
        0x2555007d04b4af7e, // finding_dialog
        0x1e79da4bfd5d66d3, // finding_dialog_error
        0x639533bebd3c7bac, // comment_dialog_shot
        0x18141fc691023af8, // comment_dialog_pick
    ],
    // Dark at 2
    [
        0xb5f8dd1af5860d36, // tab_findings_1180
        0x5d18cdacd4d30ac4, // tab_findings_860
        0x7e9374c2a6641134, // finding_dialog
        0x77aa7d0391aee716, // finding_dialog_error
        0x11e4ffbb684e03c7, // comment_dialog_shot
        0x736c805d9ae612e2, // comment_dialog_pick
    ],
    // Light at 1
    [
        0x76b03d0a65a82830, // tab_findings_1180
        0xe7dcbe2b54dc7de2, // tab_findings_860
        0x4949b61abf41a328, // finding_dialog
        0x070288f25b66d7df, // finding_dialog_error
        0x012e4e24d396e95e, // comment_dialog_shot
        0xb01d86b28a5fe8e0, // comment_dialog_pick
    ],
    // Light at 2
    [
        0xc9cc226f3d498ef2, // tab_findings_1180
        0x3498a48112430d88, // tab_findings_860
        0x2c71b3993adbd77c, // finding_dialog
        0x64a0e0763ae02561, // finding_dialog_error
        0x9a4f63e84d760097, // comment_dialog_shot
        0xb044e053c2c57b0b, // comment_dialog_pick
    ],
];

#[test]
fn ai_reviews_findings_paint_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_FINDINGS.iter().map(|row| row.as_slice()).collect();
    let view = placed(tab_findings());
    check_each(&FINDINGS, &golden, |painter, theme, scale, surface| {
        let tab = |w: i32| {
            super::feature_tab_composed(painter, &view, (w, TAB_HEIGHT).into(), theme, scale)
        };
        let dialog = |error: bool| {
            let size = IDENTITY_OUTPUT.into();
            painter.finding_dialog(&finding_dialog(error), size, scale, theme.palette())
        };
        let size = IDENTITY_OUTPUT.into();
        hash(match surface {
            0 => tab(1180),
            1 => tab(860),
            2 => dialog(false),
            3 => dialog(true),
            4 => painter.finding_dialog(&comment_dialog_shot(), size, scale, theme.palette()),
            _ => painter.finding_dialog(&comment_dialog_pick(), size, scale, theme.palette()),
        })
    });
}

/// With `GERDR_DUMP_DIR` set, write AI review's findings there.
#[test]
fn dump_findings() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    let view = placed(tab_findings());
    for (theme, scale) in CASES {
        let tag = format!("{}_{}", theme.name(), scale);
        let px = |v: i32| (v as f64 * scale).round() as usize;
        for w in [1180, 860] {
            let tab =
                super::feature_tab_composed(&painter, &view, (w, TAB_HEIGHT).into(), theme, scale);
            dump(&dir, &format!("tab_findings_{w}_{tag}"), tab, px(w));
        }
        let size = IDENTITY_OUTPUT.into();
        let shown = finding_dialog(true);
        let panel = super::finding_dialog_layout(&shown, size).panel;
        dump(
            &dir,
            &format!("finding_dialog_error_{tag}"),
            painter.finding_dialog(&shown, size, scale, theme.palette()),
            px(panel.size.w + 2 * super::SHADOW),
        );
        for (name, shown) in [
            ("comment_dialog_shot", comment_dialog_shot()),
            ("comment_dialog_pick", comment_dialog_pick()),
        ] {
            let panel = super::finding_dialog_layout(&shown, size).panel;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.finding_dialog(&shown, size, scale, theme.palette()),
                px(panel.size.w + 2 * super::SHADOW),
            );
        }
    }
}

// ------------------------------------------------ Plan review's suggestions

/// The feature tab on Plan review with the reviewer done and a card of each
/// kind, in the page's order: the user's own (accepted), one to decide
/// citing code (one citation not found), one accepted in the user's words
/// with a comment, one rejected and closed, and a question being answered
/// with the keyboard on its field. `writing`: all decided, the final plan
/// being written from them, so the cards are only to read.
fn tab_plan_review(writing: bool) -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, Dot, NodeMark, PageButton, PageSection, StagePage};
    use crate::{agent_icons::AgentIcon, feature::Stage};
    let mut view = feature_tab();
    let line = match writing {
        true => "writing the final plan",
        false => "2 to decide",
    };
    at_stage(&mut view, 2, NodeMark::Current, line);
    view.focus = super::TabArea::Findings;
    let button = |label: &str, style| PageButton {
        label: label.into(),
        style,
    };
    let mut page = StagePage {
        primary: None,
        menu: Vec::new(),
        title: "Plan review".into(),
        about: Stage::PlanReview.about().into(),
        status: None,
        buttons: Vec::new(),
        sections: Vec::new(),
    };
    let (agent, status, dot) = match writing {
        true => (
            "claude",
            "writing the final plan \u{b7} tab 3",
            Dot::Working,
        ),
        false => ("codex", "done \u{b7} tab 3", Dot::Idle),
    };
    let page_buttons: &[(&str, ButtonStyle)] = match writing {
        true => &[
            ("Show agent", ButtonStyle::Plain),
            ("Change decisions\u{2026}", ButtonStyle::Plain),
            ("Skip review", ButtonStyle::Plain),
            ("Start over\u{2026}", ButtonStyle::Quiet),
        ],
        false => &[
            ("Review in plan", ButtonStyle::Primary),
            ("Show agent", ButtonStyle::Plain),
            ("Ask reviewer", ButtonStyle::Plain),
            ("Accept all open", ButtonStyle::Plain),
            ("Add suggestion\u{2026}", ButtonStyle::Plain),
            ("Skip review", ButtonStyle::Plain),
            ("Start over\u{2026}", ButtonStyle::Quiet),
        ],
    };
    for (label, style) in page_buttons {
        page.buttons.push(button(label, *style));
    }
    page.sections.push(PageSection::Agent {
        stage: crate::feature::Stage::PlanReview,
        icon: if writing {
            AgentIcon::Claude
        } else {
            AgentIcon::Codex
        },
        name: agent.into(),
        dot,
        status: status.into(),
        tail: Vec::new(),
        buttons: vec![if writing { 0 } else { 1 }],
        picks: Vec::new(),
        model: None,
        waits: false,
    });
    let (words, lines, decided) = match writing {
        true => (
            "4 accepted \u{b7} 1 rejected \u{b7} 1 citation not found in the code",
            vec![tab_line(
                crate::review_cards::SUGGESTIONS_WRITING,
                CheckTone::Note,
            )],
            5,
        ),
        false => (
            "2 to decide \u{b7} 2 accepted \u{b7} 1 rejected \u{b7} 1 citation not found in the code",
            Vec::new(),
            3,
        ),
    };
    // Each card as the page makes it, its buttons among the page's: while
    // deciding, by where it stands; while the final plan is written, only to
    // look. Opened as the fixture shows them, the rejected one closed.
    use crate::feature::plan_review::{Evidence, Kind, Placed, Source, State, Suggestion};
    use crate::feature::review_import::Severity;
    use crate::review_cards::{CardPage, Item, item_buttons, item_card};
    let standing = |state| if writing { State::Accepted } else { state };
    let suggestion = |id: &str, severity, kind, plan: &str, title: &str, state| Suggestion {
        id: id.into(),
        severity,
        kind,
        plan: plan.into(),
        title: title.into(),
        state,
        run: 1,
        placed: Some(Placed::Heading(1)),
        ..Suggestion::default()
    };
    let cited = |file: &str, line, found| Evidence {
        repo: "api".into(),
        file: file.into(),
        line,
        quote: String::new(),
        found: Some(found),
    };
    let suggestions = [
        Suggestion {
            source: Source::You,
            change: "Milestone 2 says the lockout message names the wait, not the reason.".into(),
            ..suggestion(
                "u1",
                Severity::Medium,
                Kind::Gap,
                "",
                "Say how a locked account is told apart from a wrong password",
                State::Accepted,
            )
        },
        Suggestion {
            why: "Everyone signed in before the change is signed out at once, and the sessions \
                  table keeps rows nobody reads."
                .into(),
            evidence: vec![
                cited("src/session.rs", 42, true),
                cited("src/store.rs", 118, false),
            ],
            change: "Add a step to Milestone 1 that carries the old sessions over, with a test."
                .into(),
            ..suggestion(
                "r1-1",
                Severity::High,
                Kind::Gap,
                "Milestone 1 \u{b7} Sign in",
                "No milestone moves the old sessions over",
                standing(State::Open),
            )
        },
        Suggestion {
            why: "A restart forgets every count, so five tries become five per restart.".into(),
            my_change: Some("Keep the count in the users table, cleared on a good sign-in.".into()),
            note: "Only the count: the timestamps can stay in memory.".into(),
            ..suggestion(
                "r1-2",
                Severity::Medium,
                Kind::Risk,
                "Milestone 2 \u{b7} Lock out",
                "The lockout clock lives in memory",
                State::Accepted,
            )
        },
        Suggestion {
            reason: "the code picks it, the plan needn't".into(),
            ..suggestion(
                "r1-3",
                Severity::Low,
                Kind::EdgeCase,
                "Milestone 1 \u{b7} Sign in",
                "Name the hash in the plan",
                State::Rejected,
            )
        },
        Suggestion {
            why: "The plan doesn't say, and the mailer isn't set up in this repository.".into(),
            note: match writing {
                true => "Not yet: a line in the log is enough for now.".into(),
                false => String::new(),
            },
            ..suggestion(
                "r1-4",
                Severity::Low,
                Kind::Question,
                "Milestone 2 \u{b7} Lock out",
                "Should a locked account get an email?",
                standing(State::Open),
            )
        },
    ];
    let mut cards = Vec::new();
    for s in &suggestions {
        let item = Item::Suggestion(s);
        let on = CardPage {
            decides: !writing,
            posts: false,
        };
        let actions = item_buttons(item, on);
        let mut card = item_card(item, false, &actions, false);
        let at = page.buttons.len();
        (card.buttons.iter_mut().chain(&mut card.comment_buttons)).for_each(|b| *b += at);
        page.buttons.extend(actions.into_iter().map(|(b, _)| b));
        card.collapsed = s.state == State::Rejected;
        cards.push(card);
    }
    // A question's comment is its answer: being written, the field is
    // empty and says so.
    let answering = &mut cards[4];
    answering.field_hint = "Your answer";
    if !writing {
        answering.field = Some(crate::text_field::Line::default());
        answering.selected = true;
    }
    let want_all = (!writing).then_some("Accept all open");
    review_sections(
        &mut page,
        "Suggestions",
        (decided, 5),
        words,
        lines,
        want_all,
        cards,
    );
    view.page = page;
    view.hints = match writing {
        true => vec![
            ("Up / Down", "suggestion"),
            ("Enter", "show in plan"),
            ("Esc", "back"),
        ],
        false => vec![
            ("Enter", "save and accept"),
            ("Shift+Enter", "new line"),
            ("Ctrl+Shift+Del", "discard"),
            ("Esc", "keep draft"),
        ],
    }
    .into_iter()
    .map(|(k, w)| (k.to_owned(), w.to_owned()))
    .collect();
    if writing {
        let first = view.page.sections.iter_mut().find_map(|s| match s {
            PageSection::Finding(card) => Some(card),
            _ => None,
        });
        if let Some(card) = first {
            card.selected = true;
        }
    }
    view
}

/// Plan review once the final plan is written: its card and text first,
/// the suggestions it came from under them.
fn tab_plan_review_final() -> super::FeatureTabView {
    use super::{CheckTone, PageSection};
    let mut view = tab_plan_review(true);
    let agent = (view.page.sections.iter())
        .position(|s| matches!(s, PageSection::Agent { .. }))
        .map_or(0, |i| i + 1);
    let plan = [
        PageSection::File {
            name: "plan-final.md".into(),
            state: tab_line("ready for review", CheckTone::Blocker),
            buttons: Vec::new(),
        },
        PageSection::Doc {
            name: "plan-final.md".into(),
            blocks: std::sync::Arc::new(doc_blocks()),
            here: true,
        },
    ];
    view.page.sections.splice(agent..agent, plan);
    view
}

/// The finding dialog rejecting a suggestion, nothing typed yet.
fn finding_dialog_reject() -> super::FindingDialogView {
    let w = crate::feature::review_items::words(crate::feature::review_items::ItemOf::Suggestion);
    super::FindingDialogView {
        title: format!("{} r1-1", w.refuse_verb),
        about: "No milestone moves the old sessions over".into(),
        fields: vec![super::FindingFieldView {
            label: "Why?",
            line: crate::text_field::Line::default(),
            placeholder: w.why_hint,
            rows: 4,
        }],
        focus: 0,
        action: w.refuse_verb,
        cancel: "Cancel",
        error: None,
        hover: None,
        keys: vec![("Shift+Enter", "new line"), ("Esc", "close")],
        shot: None,
    }
}

/// The approved plan in its Suggestions tab, each suggestion flagged on the
/// line it applies to: one to decide with the cursor on it (its buttons in
/// the suggestion's words), one accepted in the user's words with their
/// comment under it, one rejected, and the user's own over the whole file.
fn doc_suggestions() -> super::ReposView {
    use super::{DetailRow, FindingHead, Hint, ReposButton, RowMark, Tone};
    use crate::diff::LineKind::Added;
    let mut view = doc_tab(1, 1.0, Vec::new(), false);
    // Only Review: the plan is approved, and only read here.
    view.modes = None;
    let hint = |button: ReposButton, label: &str, key: &str| Hint {
        button,
        label: label.into(),
        key: key.into(),
        disabled: None,
        style: Default::default(),
    };
    let text = |number: usize, text: &str| {
        DetailRow::Line(crate::diff::Line {
            kind: Added,
            text: text.into(),
            raw: None,
            number,
            old: 0,
            words: Vec::new(),
            syntax: Vec::new(),
        })
    };
    let head = |severity: &str, state: Option<&str>, tone, place: &str| DetailRow::CommentHead {
        who: "Plan review",
        finding: Some(FindingHead {
            severity: severity.into(),
            state: state.map(Into::into),
            tone,
        }),
        place: place.into(),
        within: None,
    };
    let said = |text: &str, last, tone| DetailRow::Comment {
        text: text.into(),
        first: false,
        last,
        tone,
        within: None,
    };
    view.title = "Review in plan \u{b7} plan.md".into();
    view.summary =
        "\u{201c}Log in with a password\u{201d} \u{b7} 1 to decide \u{b7} 2 accepted \u{b7} 1 rejected"
            .into();
    view.hints = vec![
        hint(ReposButton::Comment, "comment on the suggestion", "c"),
        hint(ReposButton::CopyLines, "copy", "Ctrl+C"),
        hint(
            ReposButton::NextComment,
            "next, previous suggestion",
            "] / [",
        ),
        hint(ReposButton::Approve, "write final plan", "Ctrl+Enter"),
        hint(ReposButton::Help, "keys", "F1"),
        hint(ReposButton::Close, "back", "Esc"),
    ];
    view.detail.title = "plan.md \u{b7} 8 lines".into();
    view.detail.title_buttons = vec![
        hint(ReposButton::PrevComment, "Previous suggestion", ""),
        hint(ReposButton::NextComment, "Next suggestion", ""),
        hint(ReposButton::Approve, "Write final plan", "Ctrl+Enter"),
    ];
    super::mark_primary(&mut view.detail.title_buttons);
    view.detail.rows = vec![
        DetailRow::CommentHead {
            who: "Your suggestion",
            finding: None,
            place: "your suggestion 1 of 1".into(),
            within: None,
        },
        said(
            "Say how a locked account is told apart from a wrong password",
            false,
            None,
        ),
        said("Gap \u{b7} not placed in the plan", false, None),
        said(
            "Change: Milestone 2 says the lockout message names the wait, not the reason.",
            true,
            None,
        ),
        text(1, "# Plan: log in with a password"),
        text(2, ""),
        text(3, "## Milestone 1 \u{b7} Sign in"),
        text(
            4,
            "Add the login form, and check the password against the stored hash.",
        ),
        head("high", None, Tone::Error, "suggestion 1 of 3"),
        said(
            "No milestone moves the old sessions over",
            false,
            Some(Tone::Error),
        ),
        said(
            "Gap \u{b7} Milestone 1 \u{b7} Sign in",
            false,
            Some(Tone::Error),
        ),
        said(
            "Everyone signed in before the change is signed out at once.",
            false,
            Some(Tone::Error),
        ),
        said(
            "Change: Add a step to Milestone 1 that carries the old sessions over, with a test.",
            false,
            Some(Tone::Error),
        ),
        said(
            "api/src/store.rs:118 sessions.clear() (not found in the code)",
            true,
            Some(Tone::Error),
        ),
        text(5, ""),
        text(6, "## Milestone 2 \u{b7} Lock out"),
        text(
            7,
            "After five tries the account waits a minute, and says for how long.",
        ),
        head(
            "medium",
            Some("accepted"),
            Tone::Medium,
            "suggestion 2 of 3",
        ),
        said(
            "The lockout clock lives in memory",
            false,
            Some(Tone::Medium),
        ),
        said(
            "Your change: Keep the count in the users table, cleared on a good sign-in.",
            true,
            Some(Tone::Medium),
        ),
        DetailRow::CommentHead {
            who: "Your comment",
            finding: None,
            place: String::new(),
            within: Some(Tone::Medium),
        },
        DetailRow::Comment {
            text: "Only the count: the timestamps can stay in memory.".into(),
            first: false,
            last: true,
            tone: None,
            within: Some(Tone::Medium),
        },
        head("low", Some("rejected"), Tone::Muted, "suggestion 3 of 3"),
        said("Name the hash in the plan", false, Some(Tone::Muted)),
        said(
            "Rejected: the code picks it, the plan needn't",
            true,
            Some(Tone::Muted),
        ),
        text(8, "Tests cover both milestones."),
    ];
    let on = 8;
    view.detail.marks = (0..view.detail.rows.len())
        .map(|r| RowMark {
            selected: r == on,
            top: r == on,
            bottom: r == on,
            noted: false,
        })
        .collect();
    view.detail.row_buttons = vec![
        (on, hint(ReposButton::FixFinding, "Accept", "f")),
        (
            on,
            hint(ReposButton::DismissFinding, "Reject\u{2026}", "Del"),
        ),
        (on, hint(ReposButton::Comment, "Comment", "c")),
        (
            on,
            hint(ReposButton::EditFix, "Edit change\u{2026}", "Shift+E"),
        ),
    ];
    view.detail.ticks = vec![(0.0, false), (0.3, true), (0.66, false), (0.86, false)];
    view.detail.part = 0.0;
    view.detail.first = 0;
    view.detail.total = view.detail.rows.len();
    view.detail.caret = None;
    view
}

const PLAN_REVIEW: [&str; 7] = [
    "tab_plan_review_cards_1180",
    "tab_plan_review_cards_860",
    "tab_plan_review_writing_1180",
    "doc_suggestions_1180",
    "doc_suggestions_850",
    "finding_dialog_reject",
    "tab_plan_review_final_1180",
];

/// The recorded hashes of `PLAN_REVIEW`, per case in `CASES`.
const GOLDEN_PLAN_REVIEW: [[u64; 7]; 4] = [
    // Dark at 1
    [
        0x24269a8e2d697d06, // tab_plan_review_cards_1180
        0x0af86ad4096e7aa3, // tab_plan_review_cards_860
        0x6ccfa4265f165d02, // tab_plan_review_writing_1180
        0xf2ed7f905e36a836, // doc_suggestions_1180
        0xa2d01c7198efce5a, // doc_suggestions_850
        0xef29518e2dc79076, // finding_dialog_reject
        0x01f089ca58405009, // tab_plan_review_final_1180
    ],
    // Dark at 2
    [
        0x3be7c5bcd9704564, // tab_plan_review_cards_1180
        0x2a40b4c0b7d35913, // tab_plan_review_cards_860
        0x153c19cd64a98b8a, // tab_plan_review_writing_1180
        0x4a3da7479ece1f73, // doc_suggestions_1180
        0xd0ff3ff64d0737ab, // doc_suggestions_850
        0xb606ef86798ca204, // finding_dialog_reject
        0x71504de1609945c5, // tab_plan_review_final_1180
    ],
    // Light at 1
    [
        0xffeb075c8078ae35, // tab_plan_review_cards_1180
        0x66ac9b6b468d1d01, // tab_plan_review_cards_860
        0xaf66d91dc29b33df, // tab_plan_review_writing_1180
        0x5fb392f855584e45, // doc_suggestions_1180
        0x2d929b2a87c88b65, // doc_suggestions_850
        0x6b76f1508ea97daa, // finding_dialog_reject
        0x886b4f096f2d5b9a, // tab_plan_review_final_1180
    ],
    // Light at 2
    [
        0x41a5862c355935a5, // tab_plan_review_cards_1180
        0x84fac7e43dbb6e7e, // tab_plan_review_cards_860
        0x8dd7af520de60217, // tab_plan_review_writing_1180
        0x2d7a66fa40ec984f, // doc_suggestions_1180
        0x99b85378c2ada16f, // doc_suggestions_850
        0x31b7feb26a182cfc, // finding_dialog_reject
        0x1ca653a53ce94e81, // tab_plan_review_final_1180
    ],
];

/// Plan review's surface `i` of `PLAN_REVIEW`, and how wide it is.
fn plan_review_surface(
    painter: &Painter,
    theme: Theme,
    scale: f64,
    i: usize,
) -> (MemoryRenderBuffer, i32) {
    let tab = |view: &super::FeatureTabView, w: i32| {
        let buffer = super::feature_tab_composed(
            painter,
            &placed(view.clone()),
            (w, TAB_HEIGHT).into(),
            theme,
            scale,
        );
        (buffer, w)
    };
    match i {
        0 => tab(&tab_plan_review(false), 1180),
        // Scrolled to the question being answered, and the run's card.
        1 => {
            let mut view = tab_plan_review(false);
            view.scroll[super::TabColumn::Page as usize] = 560.0;
            tab(&view, 860)
        }
        2 => tab(&tab_plan_review(true), 1180),
        6 => tab(&tab_plan_review_final(), 1180),
        3 => (
            doc_tab_buffer(painter, &doc_suggestions(), 1180, theme, scale),
            1180 + 2 * super::SHADOW,
        ),
        4 => (
            doc_tab_buffer(painter, &doc_suggestions(), 850, theme, scale),
            850 + 2 * super::SHADOW,
        ),
        _ => {
            let size = IDENTITY_OUTPUT.into();
            let shown = finding_dialog_reject();
            let panel = super::finding_dialog_layout(&shown, size).panel;
            (
                painter.finding_dialog(&shown, size, scale, theme.palette()),
                panel.size.w + 2 * super::SHADOW,
            )
        }
    }
}

#[test]
fn plan_reviews_suggestions_paint_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_PLAN_REVIEW
        .iter()
        .map(|row| row.as_slice())
        .collect();
    check_each(&PLAN_REVIEW, &golden, |painter, theme, scale, i| {
        hash(plan_review_surface(painter, theme, scale, i).0)
    });
}

/// With `GERDR_DUMP_DIR` set, write Plan review's suggestions there.
#[test]
fn dump_plan_review() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    for (theme, scale) in CASES {
        for (i, name) in PLAN_REVIEW.iter().enumerate() {
            let (buffer, w) = plan_review_surface(&painter, theme, scale, i);
            let px = (w as f64 * scale).round() as usize;
            dump(
                &dir,
                &format!("{name}_{}_{}", theme.name(), scale),
                buffer,
                px,
            );
        }
    }
}

/// A feature's brief in its tab, as `doc_tab_view` lays it out: the mode
/// switch on the title row, the finishing actions at the foot.
fn doc_tab(on: usize, share: f64, off: Vec<usize>, wide: bool) -> super::ReposView {
    use super::{DetailRow, DetailView, Hint, ModeSwitch, ReposButton, ReposView};
    use crate::diff::LineKind::Added;
    let hint = |button: ReposButton, key: &str| Hint {
        button,
        label: button.label().into(),
        key: key.into(),
        disabled: None,
        style: Default::default(),
    };
    let text = [
        "# Log in with a password",
        "",
        "People sign in with an email and a password, and stay signed in.",
        "A wrong password says so, without saying which of the two it was. After",
        "five tries the account waits a minute, and says for how long.",
    ];
    // The last line goes on over two rows, the later with no number.
    let rows: Vec<DetailRow> = (text.iter().enumerate())
        .map(|(i, line)| {
            DetailRow::Line(crate::diff::Line {
                kind: Added,
                text: (*line).into(),
                raw: None,
                number: if i == 4 { 0 } else { i + 1 },
                old: 0,
                words: Vec::new(),
                syntax: Vec::new(),
            })
        })
        .collect();
    let mut hints = vec![
        hint(ReposButton::SendBack, "Ctrl+Shift+Enter"),
        hint(ReposButton::Approve, "Ctrl+Enter"),
        hint(ReposButton::Comment, "c"),
        hint(ReposButton::Help, "F1"),
        hint(ReposButton::Close, "Esc"),
    ];
    let mut detail = match on {
        // Preview: the page has it all, comments take a line to go on.
        0 => {
            hints.retain(|h| h.button != ReposButton::Comment);
            DetailView::default()
        }
        _ => DetailView {
            title: "brief.md \u{b7} 4 lines".into(),
            total: rows.len(),
            gutter: 2,
            plain: true,
            title_buttons: vec![
                hint(ReposButton::CommentFile, "Shift+C"),
                hint(ReposButton::SendBack, "Ctrl+Shift+Enter"),
                hint(ReposButton::Approve, "Ctrl+Enter"),
            ],
            caret: Some((0, 0)),
            focused: true,
            rows,
            ..Default::default()
        },
    };
    super::mark_primary(&mut detail.title_buttons);
    let mut labels = vec!["Preview".to_string(), "Review".to_string()];
    if wide {
        labels.push("Split".to_string());
    }
    let title = "Brief \u{b7} Log in with a password".to_string();
    let summary = "ready for review".to_string();
    let density = Default::default();
    ReposView {
        flat: true,
        detail_share: share,
        modes: Some(ModeSwitch { labels, on, off }),
        ..ReposView::diff_only(title, summary, None, hints, detail, None, density)
    }
}

/// The brief, approved: only read, it reopens until the plan is written
/// (`locked`), and goes on to Plan either way.
fn doc_tab_closed(locked: bool) -> super::ReposView {
    use super::{Hint, ReposButton};
    let mut view = doc_tab(1, 1.0, Vec::new(), false);
    let hint = |button: ReposButton, key: &str, label: &str| Hint {
        button,
        label: label.into(),
        key: key.into(),
        disabled: None,
        style: Default::default(),
    };
    let go_on = hint(ReposButton::GoOn, "Ctrl+Enter", "Go to Plan");
    let reopen = hint(ReposButton::Reopen, "Ctrl+Shift+Enter", "Reopen");
    let (mut buttons, mut hints) = (vec![go_on.clone()], vec![go_on]);
    if !locked {
        buttons.insert(0, reopen.clone());
        hints.insert(0, reopen);
    }
    hints.push(hint(ReposButton::Help, "F1", ReposButton::Help.label()));
    hints.push(hint(ReposButton::Close, "Esc", ReposButton::Close.label()));
    view.hints = hints;
    view.detail.title_buttons = buttons;
    super::mark_primary(&mut view.detail.title_buttons);
    view.summary = match locked {
        true => "approved \u{b7} locked: Plan has plan.md".into(),
        false => "approved 1h ago by you".into(),
    };
    view.summary_tone = crate::chrome::Tone::Success;
    // A brief's tab has Review and Split only, and this one isn't wide enough
    // for Split.
    view.modes = None;
    view
}

/// A Rust file's diff in its code's colours: a comment opened on the
/// removed side only, a changed line with its words marked, the cursor's
/// row lit with a selection, the caret, a place found and a comment's dot.
fn code_diff() -> super::ReposView {
    use super::{DetailRow, RowMark};
    let mut view = doc_tab(1, 1.0, Vec::new(), false);
    let text = [
        "diff --git a/src/git.rs b/src/git.rs",
        "--- a/src/git.rs",
        "+++ b/src/git.rs",
        "@@ -10,7 +10,6 @@ impl Repo {",
        "     /// The branch checked out, if any.",
        "     pub fn branch(&self) -> Option<String> {",
        "-        let head = std::fs::read_to_string(\"HEAD\").ok()?;",
        "-        /* old: read HEAD",
        "-           by hand */",
        "+        let head = std::fs::read_to_string(\".git/HEAD\").ok()?;",
        "+        // Detached: no branch.",
        "         Some(head.trim().to_owned())",
        "     }",
    ];
    let file = crate::diff::parse(&(text.join("\n") + "\n")).remove(0);
    let hunk = &file.hunks[0];
    let mut rows = vec![DetailRow::Hunk {
        context: hunk.context.clone(),
        number: 10,
    }];
    rows.extend(hunk.lines.iter().cloned().map(DetailRow::Line));
    let n = rows.len();
    view.detail.title = "src/git.rs \u{b7} +2 \u{2212}3".into();
    view.detail.plain = false;
    view.detail.total = n;
    view.detail.caret = Some((4, 16));
    view.detail.chosen = vec![(4, 8..14)];
    view.detail.found = (0..n)
        .map(|r| match r {
            8 => vec![(18..22, true)],
            _ => Vec::new(),
        })
        .collect();
    view.detail.marks = (0..n)
        .map(|r| RowMark {
            selected: r == 4,
            top: r == 4,
            bottom: r == 4,
            noted: r == 7,
        })
        .collect();
    view.title = "Code review \u{b7} src/git.rs".into();
    view.modes = None;
    view.detail.rows = rows;
    view
}

/// Each doc tab fixture and the width of content it is seen at.
/// A code review with a comment of the user's and an AI review's findings
/// in each of their looks, the cursor on the first finding.
fn findings_view() -> super::ReposView {
    use super::{DetailRow, FindingHead, Hint, ReposButton, RowMark, Tone};
    let mut view = doc_tab(1, 1.0, Vec::new(), false);
    let hint = |button: ReposButton, label: &str| Hint {
        button,
        label: label.into(),
        key: String::new(),
        disabled: None,
        style: Default::default(),
    };
    let head = |severity: &str, state: Option<&str>, tone, place: &str| DetailRow::CommentHead {
        who: "AI review",
        finding: Some(FindingHead {
            severity: severity.into(),
            state: state.map(Into::into),
            tone,
        }),
        place: place.into(),
        within: None,
    };
    let line = |text: &str, last, tone| DetailRow::Comment {
        text: text.into(),
        first: false,
        last,
        tone,
        within: None,
    };
    view.detail.title =
        "Bulk export \u{b7} 3 files \u{b7} +3 \u{2212}1 \u{b7} 4 findings, 2 open".into();
    view.detail.title_buttons = vec![
        hint(ReposButton::PrevComment, "Previous comment"),
        hint(ReposButton::NextComment, "Next comment"),
    ];
    view.detail.rows = vec![
        DetailRow::CommentHead {
            who: "Your comment",
            finding: None,
            place: "comment 1 of 1".into(),
            within: None,
        },
        line("Why status here? Check the caller.", true, None),
        head("high", None, Tone::Error, "finding 1 of 4"),
        line("The parser splits on spaces", false, Some(Tone::Error)),
        line("Fix: split on NUL", true, Some(Tone::Error)),
        head("medium", Some("to fix"), Tone::Medium, "finding 2 of 4"),
        line("No timeout on the status read", false, Some(Tone::Medium)),
        DetailRow::CommentHead {
            who: "Your comment",
            finding: None,
            place: String::new(),
            within: Some(Tone::Medium),
        },
        DetailRow::Comment {
            text: "Keep it under a second".into(),
            first: false,
            last: true,
            tone: None,
            within: Some(Tone::Medium),
        },
        head("low", None, Tone::Normal, "finding 3 of 4"),
        line("This name says too little", false, Some(Tone::Normal)),
        DetailRow::Editor {
            text: "It reads fine to me".into(),
            caret: Some(19),
            selection: 0..0,
            first: true,
            last: true,
            within: Some(Tone::Normal),
        },
        head("low", Some("dismissed"), Tone::Muted, "finding 4 of 4"),
        line("Dismissed: it reads fine", true, Some(Tone::Muted)),
    ];
    view.detail.marks = (0..view.detail.rows.len())
        .map(|r| RowMark {
            selected: r == 2,
            top: r == 2,
            bottom: r == 2,
            noted: false,
        })
        .collect();
    view.detail.row_buttons = vec![
        (2, hint(ReposButton::FixFinding, "Fix")),
        (2, hint(ReposButton::DismissFinding, "Dismiss")),
    ];
    view.detail.ticks = vec![(0.0, false), (0.18, true), (0.45, false), (0.82, false)];
    view.detail.part = 0.0;
    view.detail.first = 0;
    view.detail.total = view.detail.rows.len();
    view
}

/// A feature's diff as each stage has it: at AI review with findings left
/// to decide (Plan fixes greyed), all decided (blue, a general comment
/// listed first), and at Code review; then Make fixes, as with nothing of the
/// user's: all decided, and with findings left at the width where the title
/// row is fullest. Those two are about the buttons: their rows are the shared
/// fixture's, comments and all.
fn findings_popup(at: u8) -> super::ReposView {
    use super::{DetailRow, Hint, ReposButton, RowMark, Tone};
    let mut view = findings_view();
    let button = |button: ReposButton, disabled: Option<&str>| Hint {
        button,
        label: button.label().into(),
        key: String::new(),
        disabled: disabled.map(Into::into),
        style: Default::default(),
    };
    let keyed = |b: ReposButton, key: &str, disabled: Option<&str>| Hint {
        key: key.into(),
        label: b.label().to_lowercase(),
        ..button(b, disabled)
    };
    let waits = Some("Decide the 2 findings left first");
    let nav = [
        button(ReposButton::PrevComment, None),
        button(ReposButton::NextComment, None),
    ];
    let (title, summary, buttons, hints): (_, _, Vec<Hint>, Vec<Hint>) = match at {
        0 => (
            "AI review of \u{201c}Bulk export\u{201d}",
            "1 to fix \u{b7} 1 dismissed \u{b7} 2 to decide \u{b7} 1 of 3 files viewed",
            vec![
                button(ReposButton::AddComment, None),
                button(ReposButton::FixAllFindings, None),
                button(ReposButton::PlanFixes, waits),
            ],
            vec![
                keyed(ReposButton::AddComment, "Shift+C", None),
                keyed(ReposButton::FixAllFindings, "Shift+F", None),
                keyed(ReposButton::PlanFixes, "Ctrl+Enter", waits),
            ],
        ),
        1 => (
            "AI review of \u{201c}Bulk export\u{201d}",
            "3 to fix \u{b7} 1 dismissed \u{b7} 0 to decide \u{b7} 1 of 3 files viewed",
            vec![
                button(ReposButton::AddComment, None),
                button(ReposButton::PlanFixes, None),
            ],
            vec![
                keyed(ReposButton::AddComment, "Shift+C", None),
                keyed(ReposButton::PlanFixes, "Ctrl+Enter", None),
            ],
        ),
        3 => (
            "AI review of \u{201c}Bulk export\u{201d}",
            "3 to fix \u{b7} 1 dismissed \u{b7} 0 to decide \u{b7} 1 of 3 files viewed",
            vec![
                button(ReposButton::AddComment, None),
                button(ReposButton::MakeFixes, None),
                button(ReposButton::PlanFixes, None),
            ],
            vec![
                keyed(ReposButton::AddComment, "Shift+C", None),
                keyed(ReposButton::MakeFixes, "Ctrl+Enter", None),
                keyed(ReposButton::PlanFixes, "Ctrl+Shift+Enter", None),
            ],
        ),
        4 => (
            "AI review of \u{201c}Bulk export\u{201d}",
            "1 to fix \u{b7} 1 dismissed \u{b7} 2 to decide \u{b7} 1 of 3 files viewed",
            vec![
                button(ReposButton::AddComment, None),
                button(ReposButton::FixAllFindings, None),
                button(ReposButton::PlanFixes, waits),
            ],
            vec![
                keyed(ReposButton::AddComment, "Shift+C", None),
                keyed(ReposButton::FixAllFindings, "Shift+F", None),
                keyed(ReposButton::PlanFixes, "Ctrl+Shift+Enter", waits),
            ],
        ),
        _ => (
            "Code review of \u{201c}Bulk export\u{201d}",
            "ready for review \u{b7} 3 of 3 files viewed",
            vec![
                button(ReposButton::AddComment, None),
                button(ReposButton::PlanFixes, Some("Add a comment first")),
                button(ReposButton::Approve, None),
            ],
            vec![
                keyed(ReposButton::AddComment, "Shift+C", None),
                keyed(
                    ReposButton::PlanFixes,
                    "Ctrl+Shift+Enter",
                    Some("Add a comment first"),
                ),
                keyed(ReposButton::Approve, "Ctrl+Enter", None),
            ],
        ),
    };
    view.title = title.to_string();
    view.summary = summary.to_string();
    view.summary_tone = Tone::Normal;
    view.modes = None;
    view.detail.title = match at {
        0 => "Since its base \u{b7} 3 files \u{b7} +3 \u{2212}1 \u{b7} 4 findings, 2 open",
        _ => "Since its base \u{b7} 3 files \u{b7} +3 \u{2212}1 \u{b7} 4 findings",
    }
    .into();
    view.detail.title_buttons = nav.into_iter().chain(buttons).collect();
    super::mark_primary(&mut view.detail.title_buttons);
    view.hints = hints;
    view.hints.push(keyed(ReposButton::Help, "F1", None));
    view.hints.push(keyed(ReposButton::Close, "Esc", None));
    if at == 1 {
        let mut rows = vec![
            DetailRow::Hunk {
                context: "General comments".into(),
                number: 0,
            },
            DetailRow::CommentHead {
                who: "Your comment",
                finding: None,
                place: "comment 1 of 1".into(),
                within: None,
            },
            DetailRow::Comment {
                text: "Log the export's size as well".into(),
                first: false,
                last: true,
                tone: None,
                within: None,
            },
        ];
        rows.extend(view.detail.rows.iter().skip(2).cloned());
        view.detail.marks = vec![RowMark::default(); rows.len()];
        view.detail.total = rows.len();
        view.detail.rows = rows;
        view.detail.row_buttons = Vec::new();
    }
    view
}

fn doc_tabs() -> [(&'static str, super::ReposView, i32); 17] {
    [
        ("doc_preview_1180", doc_tab(0, 0.0, Vec::new(), false), 1180),
        ("doc_review_1180", doc_tab(1, 1.0, Vec::new(), false), 1180),
        (
            "doc_review_no_browser",
            doc_tab(1, 1.0, vec![0, 2], true),
            1920,
        ),
        ("doc_preview_1920", doc_tab(0, 0.0, Vec::new(), true), 1920),
        ("doc_split_1920", doc_tab(2, 0.45, Vec::new(), true), 1920),
        ("doc_review_850", doc_tab(1, 1.0, Vec::new(), false), 850),
        ("doc_approved_1180", doc_tab_closed(false), 1180),
        ("doc_locked_1180", doc_tab_closed(true), 1180),
        ("findings_1180", findings_view(), 1180),
        ("popup_deciding_1180", findings_popup(0), 1180),
        ("popup_decided_1180", findings_popup(1), 1180),
        ("popup_code_review_850", findings_popup(2), 850),
        ("popup_no_comments_1180", findings_popup(3), 1180),
        ("popup_no_comments_850", findings_popup(4), 850),
        ("popup_listed_1180", listed_review(false), 1180),
        ("inline_listed_1180", listed_review(true), 1180),
        ("code_diff_1180", code_diff(), 1180),
    ]
}

/// A feature's code review with its file list, by folder: as a popup, or
/// laid into the feature tab's page (`flat`). Files viewed, not viewed and
/// changed since, a folder partly viewed.
fn listed_review(flat: bool) -> super::ReposView {
    use super::{Check, DirLine, FileLine, LineState, RepoLine, TreeLine};
    let file = |path: &str, check, depth, added, removed, comments| {
        TreeLine::File(FileLine {
            viewed: check == Check::On,
            check,
            code: 'M',
            path: path.into(),
            conflicted: false,
            depth,
            added,
            removed,
            edited: false,
            comments,
        })
    };
    let dir = |name: &str, check, depth| {
        TreeLine::Dir(DirLine {
            check,
            open: true,
            name: name.into(),
            depth,
        })
    };
    let lines = vec![
        TreeLine::Repo(RepoLine {
            check: Check::Partial,
            expanded: Some(true),
            label: "api".into(),
            branch: Some("feature/login".into()),
            dirty: false,
            ahead: 0,
            behind: 0,
            no_upstream: false,
            stashes: 0,
            files: 4,
            added: 62,
            removed: 18,
            chip: None,
            state: LineState::Settled,
            depth: 0,
            repos: 0,
        }),
        dir("src/auth", Check::Partial, 0),
        file("login.rs", Check::On, 1, 40, 12, 2),
        file("session.rs", Check::Partial, 1, 9, 2, 0),
        dir("tests", Check::On, 0),
        file("login.rs", Check::On, 1, 12, 0, 0),
        file("Cargo.toml", Check::Off, 0, 1, 4, 0),
    ];
    let total = lines.len();
    super::ReposView {
        diff_tree: true,
        tree_nav: Some(true),
        tree_note: Some(("12 of 31 files left".into(), super::Tone::Warning)),
        flat,
        no_header: flat,
        tree_share: super::REVIEW_TREE_SHARE,
        grouping: crate::repos::Grouping::Folders,
        lines,
        total,
        selected: Some(3),
        ..findings_popup(2)
    }
}

const DOC_TAB: [&str; 17] = [
    "doc_preview_1180",
    "doc_review_1180",
    "doc_review_no_browser",
    "doc_preview_1920",
    "doc_split_1920",
    "doc_review_850",
    "doc_approved_1180",
    "doc_locked_1180",
    "findings_1180",
    "popup_deciding_1180",
    "popup_decided_1180",
    "popup_code_review_850",
    "popup_no_comments_1180",
    "popup_no_comments_850",
    "popup_listed_1180",
    "inline_listed_1180",
    "code_diff_1180",
];

/// A doc tab's buffer, laid over the whole of a `w` wide content area.
fn doc_tab_buffer(
    painter: &Painter,
    view: &super::ReposView,
    w: i32,
    theme: Theme,
    scale: f64,
) -> MemoryRenderBuffer {
    let area = smithay::utils::Rectangle::from_size((w, TAB_HEIGHT).into());
    painter.repos(view, area, scale, theme.page_palette())
}

fn paint_doc_tab(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    (doc_tabs().iter())
        .map(|(_, view, w)| hash(doc_tab_buffer(painter, view, *w, theme, scale)))
        .collect()
}

/// The recorded hashes of `DOC_TAB`, per case in `CASES`.
const GOLDEN_DOC_TAB: [[u64; 17]; 4] = [
    // Dark at 1
    [
        0x3e3caa00b704f035, // doc_preview_1180
        0x5f0eea2202df9b97, // doc_review_1180
        0xca9d36ec30eeccaf, // doc_review_no_browser
        0x0463a0b03be2a38c, // doc_preview_1920
        0x114502515cb1a37f, // doc_split_1920
        0x5f9babd357bac1db, // doc_review_850
        0x216599b1146b09da, // doc_approved_1180
        0xf29ace1fac1c7802, // doc_locked_1180
        0x5f5bdeeb94ddb410, // findings_1180
        0x7c1a9ef835d39acf, // popup_deciding_1180
        0x03e2b99959154a51, // popup_decided_1180
        0xfd878456471b85eb, // popup_code_review_850
        0x80c13d0f08e4d4a5, // popup_no_comments_1180
        0xa984852d813c6db5, // popup_no_comments_850
        0xf80132fb8f3fe8a2, // popup_listed_1180
        0x799bfe46156fc713, // inline_listed_1180
        0x1506713ac3716265, // code_diff_1180
    ],
    // Dark at 2
    [
        0x292ac6088287de97, // doc_preview_1180
        0x5da6716a91f71517, // doc_review_1180
        0x266066e6b0250b3b, // doc_review_no_browser
        0xc2ddd112e79328e1, // doc_preview_1920
        0x06ee5eb2e0540777, // doc_split_1920
        0x491594bd2e15d83f, // doc_review_850
        0xf5e8d18ed258bd89, // doc_approved_1180
        0xd5340281fdc88c84, // doc_locked_1180
        0x9371cb4a587bc69f, // findings_1180
        0x8d8bfafceddbc233, // popup_deciding_1180
        0x39aad7546ee10308, // popup_decided_1180
        0x092a355cf749eef9, // popup_code_review_850
        0xb5ca82f869c8f1f2, // popup_no_comments_1180
        0x64c028cba67c38a7, // popup_no_comments_850
        0xb5d0388c18b70338, // popup_listed_1180
        0xedadca0d6059a1bc, // inline_listed_1180
        0xf02780bb56cf0ecc, // code_diff_1180
    ],
    // Light at 1
    [
        0x1b7f0b8bd316796d, // doc_preview_1180
        0x4ef27507c8d38c48, // doc_review_1180
        0x0694f0ceb17f3448, // doc_review_no_browser
        0x394a68b69f77168f, // doc_preview_1920
        0x1b572e5fe4c415c0, // doc_split_1920
        0x0927fe767505b3c0, // doc_review_850
        0x5349bbebd2386ff2, // doc_approved_1180
        0xf4b2339be9c1a333, // doc_locked_1180
        0xe32dcef3c68bc2a2, // findings_1180
        0x471eff6a3da053c0, // popup_deciding_1180
        0x5209eeb530963413, // popup_decided_1180
        0xcbbec28bda8e7080, // popup_code_review_850
        0x58ac9030b260d16a, // popup_no_comments_1180
        0x3f438765bc7c1b2d, // popup_no_comments_850
        0xd64d70db828f6643, // popup_listed_1180
        0x126bd42ab5ecebf3, // inline_listed_1180
        0x8b82a85eb8d65ac0, // code_diff_1180
    ],
    // Light at 2
    [
        0xab656728d65fc54f, // doc_preview_1180
        0x6af7e329886f8680, // doc_review_1180
        0x0fcabb5c88553110, // doc_review_no_browser
        0x8fc4208ea0a15206, // doc_preview_1920
        0x424849e04413924d, // doc_split_1920
        0x149009729a0727b0, // doc_review_850
        0xed274b1787f3027b, // doc_approved_1180
        0xa15efb2532d0eda3, // doc_locked_1180
        0x750268505a34a269, // findings_1180
        0xa286fc3b0c1d776b, // popup_deciding_1180
        0x80fe18b5024d4622, // popup_decided_1180
        0xf45f0410b01bca4f, // popup_code_review_850
        0x89d6f942396f11e6, // popup_no_comments_1180
        0x82f7e1c5f8a0e0ea, // popup_no_comments_850
        0x55714f9f31d8664b, // popup_listed_1180
        0xe28cd76b2fd88514, // inline_listed_1180
        0xa98de4b41ec7cb9b, // code_diff_1180
    ],
];

#[test]
fn the_doc_tab_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_DOC_TAB.iter().map(|row| row.as_slice()).collect();
    check(&DOC_TAB, &golden, paint_doc_tab);
}

/// With `GERDR_DUMP_DIR` set, write every doc tab fixture there.
#[test]
fn dump_doc_tab() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    for (theme, scale) in CASES {
        for (name, view, w) in doc_tabs() {
            let width = ((w + 2 * super::SHADOW) as f64 * scale).round() as usize;
            let buffer = doc_tab_buffer(&painter, &view, w, theme, scale);
            dump(
                &dir,
                &format!("{name}_{}_{}", theme.name(), scale),
                buffer,
                width,
            );
        }
    }
}

const REVIEW_VIEWS: [&str; 6] = [
    "tab_review_code_1180",
    "tab_review_fixes_1180",
    "tab_review_fixes_860",
    "tab_review_ready_1180",
    "tab_review_code_860",
    "tab_review_done_1180",
];

/// The recorded hashes of `REVIEW_VIEWS`, per case in `CASES`.
const GOLDEN_REVIEW_VIEWS: [[u64; 6]; 4] = [
    // Dark at 1
    [
        0x62f6f85f3a60b617, // tab_review_code_1180
        0x85bc729394e8f1ce, // tab_review_fixes_1180
        0xcd92bd515ca8867a, // tab_review_fixes_860
        0xe05245efbb5b11e7, // tab_review_ready_1180
        0xc94990ce159c02a0, // tab_review_code_860
        0x3668bc5f50af56a4, // tab_review_done_1180
    ],
    // Dark at 2
    [
        0xf7f60c588b7eb13f, // tab_review_code_1180
        0xdd849879fb6d4d08, // tab_review_fixes_1180
        0x8627240adb1eada6, // tab_review_fixes_860
        0xf894411f017c2d28, // tab_review_ready_1180
        0xa73fb1716ae29198, // tab_review_code_860
        0x59df8f706350c267, // tab_review_done_1180
    ],
    // Light at 1
    [
        0xb08db80788540323, // tab_review_code_1180
        0xdf7f19326facf602, // tab_review_fixes_1180
        0xb0af4749c60070e4, // tab_review_fixes_860
        0xcccc4a32c90995f6, // tab_review_ready_1180
        0x9c31cafd6be51f40, // tab_review_code_860
        0x0713518e4a40ffd4, // tab_review_done_1180
    ],
    // Light at 2
    [
        0xec449ce8d5e4036d, // tab_review_code_1180
        0x5e151559414544ee, // tab_review_fixes_1180
        0x29f3ff7ca98c3c6d, // tab_review_fixes_860
        0x8b101ecc2f108a00, // tab_review_ready_1180
        0xb458c47bada68cff, // tab_review_code_860
        0xb60e1fe2694fb7ff, // tab_review_done_1180
    ],
];

#[test]
fn code_reviews_views_paint_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_REVIEW_VIEWS
        .iter()
        .map(|row| row.as_slice())
        .collect();
    let views = [
        (placed(feature_tab_review_code()), 1180),
        (placed(feature_tab_review_fixes()), 1180),
        (placed(feature_tab_review_fixes()), 860),
        (placed(feature_tab_review_ready()), 1180),
        (placed(feature_tab_review_code()), 860),
        (placed(feature_tab_review_done()), 1180),
    ];
    check_each(&REVIEW_VIEWS, &golden, |painter, theme, scale, surface| {
        let (view, w) = &views[surface];
        hash(super::feature_tab_composed(
            painter,
            view,
            (*w, TAB_HEIGHT).into(),
            theme,
            scale,
        ))
    });
}

/// With `GERDR_DUMP_DIR` set, write Code review's views there.
#[test]
fn dump_review_views() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    let views = [
        (placed(feature_tab_review_code()), 1180),
        (placed(feature_tab_review_fixes()), 1180),
        (placed(feature_tab_review_fixes()), 860),
        (placed(feature_tab_review_ready()), 1180),
        (placed(feature_tab_review_code()), 860),
        (placed(feature_tab_review_done()), 1180),
    ];
    for (theme, scale) in CASES {
        for ((view, w), name) in views.iter().zip(REVIEW_VIEWS) {
            let width = (*w as f64 * scale).round() as usize;
            let buffer =
                super::feature_tab_composed(&painter, view, (*w, TAB_HEIGHT).into(), theme, scale);
            dump(
                &dir,
                &format!("{name}_{}_{}", theme.name(), scale),
                buffer,
                width,
            );
        }
    }
}

const CHECK_CARDS: [&str; 2] = ["tab_checks_1180", "tab_checks_860"];

/// The recorded hashes of `CHECK_CARDS`, per case in `CASES`.
const GOLDEN_CHECK_CARDS: [[u64; 2]; 4] = [
    // Dark at 1
    [
        0x774d033f105a04a7, // tab_checks_1180
        0xb2e27164e9a4b8c5, // tab_checks_860
    ],
    // Dark at 2
    [
        0x7edffdc4eb85b021, // tab_checks_1180
        0x43296d0e2ff0fc5f, // tab_checks_860
    ],
    // Light at 1
    [
        0x0f1acca039e10808, // tab_checks_1180
        0x7fadc0f0d3b7cdc6, // tab_checks_860
    ],
    // Light at 2
    [
        0x83060c9ab69c2a9b, // tab_checks_1180
        0x8fcc9a9c3527c826, // tab_checks_860
    ],
];

#[test]
fn verifys_check_cards_paint_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_CHECK_CARDS
        .iter()
        .map(|row| row.as_slice())
        .collect();
    let view = placed(tab_checks());
    check_each(&CHECK_CARDS, &golden, |painter, theme, scale, surface| {
        let w = if surface == 0 { 1180 } else { 860 };
        hash(super::feature_tab_composed(
            painter,
            &view,
            (w, TAB_HEIGHT).into(),
            theme,
            scale,
        ))
    });
}

/// With `GERDR_DUMP_DIR` set, write Verify's check cards there.
#[test]
fn dump_check_cards() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    let view = placed(tab_checks());
    for (theme, scale) in CASES {
        let tag = format!("{}_{}", theme.name(), scale);
        let px = |v: i32| (v as f64 * scale).round() as usize;
        for w in [1180, 860] {
            let tab =
                super::feature_tab_composed(&painter, &view, (w, TAB_HEIGHT).into(), theme, scale);
            dump(&dir, &format!("tab_checks_{w}_{tag}"), tab, px(w));
        }
    }
}

// ------------------------------------------------------------ AI work stats

/// An output as small as the panel fits on.
const STATS_OUTPUT: (i32, i32) = (870, 720);

/// The stats panel with nothing kept, read, in a feature's detail, after
/// a failed read, and on its Agents tab, each from a fake index view.
fn stats_views() -> [super::StatsView; 6] {
    use crate::ai_work::stats::View;
    use crate::stats_panel::fixtures;
    let empty = fixtures::panel(|scope| {
        Some(Ok(View {
            scope,
            ..View::default()
        }))
    });
    let loaded = fixtures::panel(|scope| Some(Ok(fixtures::view(scope))));
    let detail = fixtures::detail_panel();
    let error = fixtures::panel(|_| Some(Err("database disk image is malformed".into())));
    let mut agents = fixtures::panel(|_| None);
    agents.scope.tab = crate::ai_work::stats::Tab::Agents;
    agents.view = Some(Ok(fixtures::view(agents.scope.clone())));
    // The pointer on the second feature's trash button.
    let mut delete = fixtures::panel(|scope| Some(Ok(fixtures::view(scope))));
    delete.hover = Some(super::StatsHit::Delete(1));
    [empty, loaded, detail, error, agents, delete].map(|panel| panel.view(fixtures::NOW))
}

const STATS: [&str; 6] = [
    "stats_empty",
    "stats_loaded",
    "stats_detail",
    "stats_error",
    "stats_agents",
    "stats_delete_hover",
];

fn paint_stats(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    stats_views()
        .iter()
        .map(|view| hash(painter.stats_panel(view, STATS_OUTPUT.into(), scale, p)))
        .collect()
}

/// The recorded hashes of `STATS`, per case in `CASES`.
const GOLDEN_STATS: [[u64; 6]; 4] = [
    // Dark at 1
    [
        0x35d1059f172356c9, // stats_empty
        0x50029da389638c40, // stats_loaded
        0x7cdfa462b7f66e4a, // stats_detail
        0x9b0627d7c1c5082b, // stats_error
        0x51cd2eb8e083fcff, // stats_agents
        0xa9df8f7fd96d2087, // stats_delete_hover
    ],
    // Dark at 2
    [
        0x381f35560faed9ed, // stats_empty
        0x5fa1e92ee2f4a9b4, // stats_loaded
        0x1a07a3a315649704, // stats_detail
        0x488958964b5b7982, // stats_error
        0x89f7a8d17a37269d, // stats_agents
        0xc18a73063d541e33, // stats_delete_hover
    ],
    // Light at 1
    [
        0xa798413b3103dd56, // stats_empty
        0x41fb4694679898d7, // stats_loaded
        0x5f51c34f053ea030, // stats_detail
        0xc68f2c57acd73b62, // stats_error
        0x40e9c08547ce895c, // stats_agents
        0xddfa95b1bf75c7cf, // stats_delete_hover
    ],
    // Light at 2
    [
        0xddea805ca84232c5, // stats_empty
        0xa687a9cfb7312f98, // stats_loaded
        0x89917e697a9f179d, // stats_detail
        0x753e6f522228735a, // stats_error
        0xc8878410d1f3322e, // stats_agents
        0xdab45321c3439c3b, // stats_delete_hover
    ],
];

#[test]
fn the_stats_panel_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_STATS.iter().map(|row| row.as_slice()).collect();
    check(&STATS, &golden, paint_stats);
}

/// With `GERDR_DUMP_DIR` set, write the stats panel there.
#[test]
fn dump_stats() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    let size = STATS_OUTPUT.into();
    for (theme, scale) in CASES {
        let p = theme.palette();
        let px = |v: i32| (v as f64 * scale).round() as usize;
        let tag = format!("{}_{}", theme.name(), scale);
        for (name, view) in STATS.iter().zip(stats_views()) {
            let panel = super::stats_layout(&view, size).panel;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.stats_panel(&view, size, scale, p),
                px(panel.size.w + 2 * super::SHADOW),
            );
        }
    }
}

/// The pane the criteria strip floats over.
const STRIP_PANE: (i32, i32) = (1000, 640);

/// The criteria strip over a sandbox's app: its row, its steps shown with
/// the pointer on Pass, a note being written with a screenshot and the
/// keys, a failure being fixed with its note, folded to its chip, and a
/// comment of the user's own with its six buttons.
fn strip_views() -> [super::CriteriaStripView; 7] {
    use super::{ButtonStyle, CriteriaStripView, StripHit, StripTag};
    let plain = |label: &str| (label.to_owned(), ButtonStyle::Plain);
    let row = CriteriaStripView {
        pane: Rect::new((0, 0).into(), STRIP_PANE.into()),
        folded: false,
        open: false,
        place: "AC-3 \u{b7} 3/7".into(),
        title: "Rows save on edit".into(),
        tag: StripTag::NotRun,
        state: String::new(),
        steps: vec![
            "Given two rows in the table".into(),
            "When I edit both and press Save".into(),
            "Then both rows show the new values after a reload".into(),
        ],
        note: String::new(),
        field: None,
        shots: String::new(),
        buttons: vec![plain("Pass"), plain("Fail\u{2026}")],
        steps_through: true,
        passed: false,
        hover: None,
        keys: false,
        hints: Vec::new(),
    };
    let hint = |k: &str, w: &str| (k.to_owned(), w.to_owned());
    let open = CriteriaStripView {
        open: true,
        hover: Some(StripHit::Button(0)),
        keys: true,
        hints: vec![
            hint("Space", "pass"),
            hint("F", "fail\u{2026}"),
            hint("Up / Down", "step"),
            hint("P", "show passed"),
            hint("C", "add issue\u{2026}"),
            hint("S", "select on page"),
            hint("Esc", "back"),
            hint("Ctrl+O", "page"),
            hint("Alt+Down", "fold"),
            hint("Alt+C", "screenshot"),
        ],
        ..row.clone()
    };
    let mut note = Line::default();
    note.text = "the second row reverts after a reload".into();
    note.caret = note.text.len();
    note.anchor = note.caret;
    let writing = CriteriaStripView {
        open: true,
        field: Some(note),
        shots: "1 screenshot".into(),
        keys: true,
        hints: vec![
            hint("Enter", "save"),
            hint("Shift+Enter", "new line"),
            hint("Esc", "keep draft"),
            hint("Ctrl+Shift+Del", "discard"),
            hint("Alt+C", "screenshot"),
        ],
        ..row.clone()
    };
    let failed = CriteriaStripView {
        open: true,
        // Stepping through passed criteria too: its check button on.
        passed: true,
        tag: StripTag::Failed,
        state: crate::feature::review_import::sent_word(false, false).into(),
        note: "the second row reverts after a reload".into(),
        buttons: vec![
            ("Edit note\u{2026}".into(), ButtonStyle::Quiet),
            plain("Pass"),
            ("Fail".into(), ButtonStyle::Failed),
        ],
        ..row.clone()
    };
    let chip = CriteriaStripView {
        folded: true,
        tag: StripTag::Passed,
        ..row.clone()
    };
    let quiet = |label: &str| (label.to_owned(), ButtonStyle::Quiet);
    let comment = CriteriaStripView {
        open: true,
        place: "T1 \u{b7} 8/8".into(),
        title: "Header row is too dark".into(),
        tag: StripTag::Failed,
        steps: Vec::new(),
        buttons: vec![
            quiet("Edit\u{2026}"),
            plain("Done"),
            ("Not done".into(), ButtonStyle::Failed),
            quiet("Screenshot"),
            quiet("Select on page"),
            quiet("Add issue\u{2026}"),
        ],
        ..row.clone()
    };
    // On a narrow pane its buttons wrap onto a second row.
    let narrow = CriteriaStripView {
        pane: Rect::new((0, 0).into(), (640, STRIP_PANE.1).into()),
        ..comment.clone()
    };
    [row, open, writing, failed, chip, comment, narrow]
}

const STRIP: [&str; 7] = [
    "strip_row",
    "strip_open",
    "strip_note",
    "strip_failed",
    "strip_chip",
    "strip_comment",
    "strip_comment_narrow",
];

fn paint_strip(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    (strip_views().iter())
        .map(|view| hash(painter.criteria_strip(view, scale, p)))
        .collect()
}

/// The recorded hashes of `STRIP`, per case in `CASES`.
const GOLDEN_STRIP: [[u64; 7]; 4] = [
    // Dark at 1
    [
        0x47a6bfdbd767971f, // strip_row
        0xb60429caa6eb78c4, // strip_open
        0xfb64a94904783562, // strip_note
        0x0529d8d7bd4fe04d, // strip_failed
        0x72bf636ed7694c60, // strip_chip
        0x5374134c2e6958a3, // strip_comment
        0xe85b1189f766d83f, // strip_comment_narrow
    ],
    // Dark at 2
    [
        0xc824e138ae7a5477, // strip_row
        0x61046cedbd0cbe03, // strip_open
        0x2eb9b769329bdefe, // strip_note
        0x35802653b4aecb78, // strip_failed
        0x24deb5724599f633, // strip_chip
        0x16f7278b70e7e8ca, // strip_comment
        0x0a8df215a9d0d776, // strip_comment_narrow
    ],
    // Light at 1
    [
        0x6123903b36708051, // strip_row
        0x04f6b9d4ceac389f, // strip_open
        0x631a2baefd2fc68f, // strip_note
        0x76dc11620da14777, // strip_failed
        0x2b63a463c98f403f, // strip_chip
        0xb71aae6788102bec, // strip_comment
        0x8fa6512f71b07868, // strip_comment_narrow
    ],
    // Light at 2
    [
        0x57fcae666ad4e9a3, // strip_row
        0x21747f44f47a55e2, // strip_open
        0x5577751c43c03172, // strip_note
        0x5eaf1bd3b70231cb, // strip_failed
        0xca59826d5baa4ecc, // strip_chip
        0xbf5f535bf3db12ff, // strip_comment
        0xb51fae4d540e49f3, // strip_comment_narrow
    ],
];

#[test]
fn criteria_strip_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_STRIP.iter().map(|row| row.as_slice()).collect();
    check(&STRIP, &golden, paint_strip);
}

/// With `GERDR_DUMP_DIR` set, write the criteria strip's looks there.
#[test]
fn dump_criteria_strip() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    for (theme, scale) in CASES {
        let p = theme.palette();
        let px = |v: i32| (v as f64 * scale).round() as usize;
        let tag = format!("{}_{}", theme.name(), scale);
        for (name, view) in STRIP.iter().zip(strip_views()) {
            let panel = super::criteria_strip_layout(&view).panel;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.criteria_strip(&view, scale, p),
                px(panel.size.w + 2 * super::SHADOW),
            );
        }
    }
}

/// A comment's screenshots on Manual test's page: one small, one whose
/// file is gone. The first is drawn here, the same each time.
fn golden_shots() -> Vec<super::SheetShot> {
    use resvg::tiny_skia;
    static DRAWN: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let path = DRAWN.get_or_init(|| {
        let path =
            std::env::temp_dir().join(format!("gerdr-golden-shot-{}.png", std::process::id()));
        let mut pixmap = tiny_skia::Pixmap::new(320, 180).expect("a pixmap");
        pixmap.fill(tiny_skia::Color::from_rgba8(0x30, 0x60, 0xa0, 0xff));
        let bar = tiny_skia::Rect::from_xywh(0.0, 0.0, 320.0, 36.0).expect("a rect");
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(0xf0, 0xf0, 0xf0, 0xff);
        pixmap.fill_rect(bar, &paint, tiny_skia::Transform::identity(), None);
        pixmap
            .save_png(&path)
            .expect("the golden screenshot written");
        path
    });
    let path = path.clone();
    vec![
        super::SheetShot {
            size: crate::shot_images::size(&path),
            path,
        },
        super::SheetShot {
            path: "/nonexistent/T1-2.png".into(),
            size: None,
        },
    ]
}

#[test]
fn a_comments_screenshots_sit_small_under_it_and_a_press_on_one_is_its_hit() {
    use super::{PageSection, features::Piece};
    let view = feature_tab_manual_comment();
    let area = Rect::new((0, 0).into(), (1180, TAB_HEIGHT).into());
    let shots = |view: &super::FeatureTabView| {
        let layout = super::feature_tab_layout(view, area);
        let rects: Vec<Rect> = (layout.pieces.iter().flatten())
            .filter_map(|(r, p)| matches!(p, Piece::Shot(..)).then_some(*r))
            .collect();
        (layout, rects)
    };
    let (layout, small) = shots(&view);
    assert_eq!(small.len(), 2);
    // Side by side, no taller than a thumbnail, the gone one 16:9.
    assert_eq!(small[0].loc.y, small[1].loc.y);
    assert!(small.iter().all(|r| r.size.h <= 72 && r.size.w <= 160));
    assert_eq!((small[1].size.w, small[1].size.h), (128, 72));
    let card = (view.page.sections.iter())
        .filter(|s| matches!(s, PageSection::Finding(_)))
        .count()
        - 1;
    let middle = |r: Rect| (r.loc.x + r.size.w / 2, r.loc.y + r.size.h / 2).into();
    assert_eq!(
        layout.hit(&view, middle(small[1])),
        Some(super::FeatureTabHit::Shot(card, 1))
    );
    // Each has a × in its top right corner, inside it, its own hit.
    let crosses: Vec<Rect> = (layout.pieces.iter().flatten())
        .filter_map(|(r, p)| matches!(p, Piece::ShotDelete(..)).then_some(*r))
        .collect();
    assert_eq!(crosses.len(), 2);
    for (x, shot) in crosses.iter().zip(&small) {
        assert!(super::inside(*shot, *x), "{x:?} in {shot:?}");
        assert!(x.loc.x + x.size.w > shot.loc.x + shot.size.w / 2);
        assert!(x.loc.y < shot.loc.y + shot.size.h / 2);
    }
    assert_eq!(
        layout.hit(&view, middle(crosses[1])),
        Some(super::FeatureTabHit::ShotDelete(card, 1))
    );
}

/// The screenshot viewer's looks: one of two, Next under the pointer; and
/// one whose file is gone.
const VIEWER: [&str; 2] = ["viewer", "viewer_gone"];

/// The output the viewer's looks are drawn for.
const VIEWER_OUTPUT: (i32, i32) = (1280, 800);

fn viewer_views() -> [super::ShotViewerView; 2] {
    let shot = golden_shots().remove(0);
    [
        super::ShotViewerView {
            path: shot.path,
            size: shot.size,
            title: "T1 \u{b7} Make the settings page look better".into(),
            at: 0,
            of: 2,
            hover: Some(super::ShotViewerHit::Next),
        },
        super::ShotViewerView {
            path: "/nonexistent/AC-2-1.png".into(),
            size: None,
            title: "AC-2 \u{b7} Export off with no invoices".into(),
            at: 0,
            of: 1,
            hover: None,
        },
    ]
}

fn paint_viewer(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    (viewer_views().iter())
        .map(|view| hash(painter.shot_viewer(view, VIEWER_OUTPUT.into(), scale, p)))
        .collect()
}

/// The recorded hashes of `VIEWER`, per case in `CASES`.
const GOLDEN_VIEWER: [[u64; 2]; 4] = [
    // Dark at 1
    [
        0x99dadb6746e3e667, // viewer
        0x97c1380bf13efc53, // viewer_gone
    ],
    // Dark at 2
    [
        0x9f09eff6f1a2950f, // viewer
        0x3ef5bff72fad9d2d, // viewer_gone
    ],
    // Light at 1
    [
        0x5cff3002b6568854, // viewer
        0x59d992a8de54492d, // viewer_gone
    ],
    // Light at 2
    [
        0xcede64bcbfbd91c8, // viewer
        0xeae0bac3f440b436, // viewer_gone
    ],
];

#[test]
fn the_shot_viewer_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_VIEWER.iter().map(|row| row.as_slice()).collect();
    check(&VIEWER, &golden, paint_viewer);
}

/// With `GERDR_DUMP_DIR` set, write the screenshot viewer's looks there.
#[test]
fn dump_shot_viewer() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    for (theme, scale) in CASES {
        let p = theme.palette();
        let px = |v: i32| (v as f64 * scale).round() as usize;
        let tag = format!("{}_{}", theme.name(), scale);
        for (name, view) in VIEWER.iter().zip(viewer_views()) {
            let panel = super::shot_viewer_layout(&view, VIEWER_OUTPUT.into()).panel;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.shot_viewer(&view, VIEWER_OUTPUT.into(), scale, p),
                px(panel.size.w + 2 * super::SHADOW),
            );
        }
    }
}

/// The Ship sheet of a feature that squashes: its steps, then its commit
/// message, filled with the title and the summary, the caret at its end.
fn ship_squash_dialog() -> super::FindingDialogView {
    let text = "Faster sidebar\n\nThe sidebar reads Git once per change, not per frame.";
    let mut line = crate::text_field::Line {
        text: text.into(),
        ..Default::default()
    };
    (line.caret, line.anchor) = (text.len(), text.len());
    super::FindingDialogView {
        title: "Ship \u{201c}Faster sidebar\u{201d}?".into(),
        about: "From feature/faster-sidebar:\nSquash into main\nPush main".into(),
        fields: vec![super::FindingFieldView {
            label: "Commit message",
            line,
            placeholder: "What the commit says",
            rows: 6,
        }],
        focus: 0,
        action: "Ship",
        cancel: "Cancel",
        error: None,
        hover: None,
        keys: vec![("Shift+Enter", "new line"), ("Esc", "close")],
        shot: None,
    }
}

const SHIP_SQUASH: [&str; 1] = ["ship_squash_dialog"];

/// The recorded hashes of `SHIP_SQUASH`, per case in `CASES`.
const GOLDEN_SHIP_SQUASH: [[u64; 1]; 4] = [
    [0x00d646dc6cb7ee41], // Dark at 1
    [0x927f398ffbaaa8f0], // Dark at 2
    [0x26ed53c998b78632], // Light at 1
    [0xa20af2fafd2a8663], // Light at 2
];

/// The Ship sheet with its message field, and how wide it is.
fn ship_squash_surface(painter: &Painter, theme: Theme, scale: f64) -> (MemoryRenderBuffer, i32) {
    let size = IDENTITY_OUTPUT.into();
    let shown = ship_squash_dialog();
    let panel = super::finding_dialog_layout(&shown, size).panel;
    (
        painter.finding_dialog(&shown, size, scale, theme.palette()),
        panel.size.w + 2 * super::SHADOW,
    )
}

#[test]
fn the_ship_sheet_with_a_message_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_SHIP_SQUASH
        .iter()
        .map(|row| row.as_slice())
        .collect();
    check_each(&SHIP_SQUASH, &golden, |painter, theme, scale, _| {
        hash(ship_squash_surface(painter, theme, scale).0)
    });
}

/// With `GERDR_DUMP_DIR` set, write the Ship sheet with its message there.
#[test]
fn dump_ship_squash() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    for (theme, scale) in CASES {
        let (buffer, w) = ship_squash_surface(&painter, theme, scale);
        let px = (w as f64 * scale).round() as usize;
        let name = format!("{}_{}_{}", SHIP_SQUASH[0], theme.name(), scale);
        dump(&dir, &name, buffer, px);
    }
}

/// A pane shown as a chat: still reading it, none found, the chat with
/// its cards (a folded and an unfolded tool, an unfolded edit, a thinking
/// line), the same while the agent waits on the user, a reply being
/// written, an item picked and another hovered, and scrolled down.
const CHATS: [&str; 8] = [
    "chat_finding",
    "chat_missing",
    "chat",
    "chat_waiting",
    "chat_composer",
    "chat_picked",
    "chat_scrolled",
    "chat_question",
];

fn chat_view(i: usize) -> super::ChatView {
    use super::agent_chat::tests::{question, screen, view};
    use super::{ChatHit, WaitKey};
    use crate::chat::{ChatFocus, Reading};
    let v = view();
    match i {
        0 => super::ChatView {
            reading: Reading::Finding,
            items: std::sync::Arc::new(Vec::new()),
            ..v
        },
        1 => super::ChatView {
            status: AgentStatus::Idle,
            reading: Reading::Missing(
                "Couldn't find this agent's chat. Its screen still works.".into(),
            ),
            items: std::sync::Arc::new(Vec::new()),
            hover: Some(ChatHit::ShowScreen),
            ..v
        },
        2 => v,
        3 => super::ChatView {
            status: AgentStatus::Blocked,
            waiting: Some(screen()),
            hover: Some(ChatHit::Key(WaitKey::Enter)),
            ..v
        },
        4 => {
            let text = "Looks good. Also log each retry\nwith its wait, at debug";
            super::ChatView {
                status: AgentStatus::Idle,
                focus: ChatFocus::Composer,
                hints: crate::chat::footer(ChatFocus::Composer, false),
                draft: Line {
                    text: text.into(),
                    caret: text.len(),
                    anchor: text.len(),
                    scroll: 0,
                },
                ..v
            }
        }
        // Picked, a fold hovered, and the commands' steps open.
        5 => {
            let mut unfolded = v.unfolded.clone();
            unfolded.insert(crate::chat::rows::steps_key("t2"));
            super::ChatView {
                picked: Some(1),
                hover: Some(ChatHit::Fold(2)),
                unfolded,
                ..v
            }
        }
        // A question of a few, its second choice hovered.
        7 => super::ChatView {
            status: AgentStatus::Blocked,
            waiting: Some(question()),
            hover: Some(ChatHit::Key(WaitKey::Digit(2))),
            ..v
        },
        // A shorter pane, for the list to scroll.
        _ => super::ChatView {
            rect: super::Rect::new(v.rect.loc, (v.rect.size.w, 460).into()),
            top: 200.0,
            ..v
        },
    }
}

fn paint_chats(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    (0..CHATS.len())
        .map(|i| hash(painter.agent_chat(&chat_view(i), scale, p)))
        .collect()
}

/// The recorded hashes of `CHATS`, per case in `CASES`.
const GOLDEN_CHATS: [[u64; 8]; 4] = [
    // Dark at 1
    [
        0xa5039d4ee1fb06ee, // chat_finding
        0x4c9a3c6fa240b9e9, // chat_missing
        0x9f85d649e034a3ad, // chat
        0xe71506c1721ec8a1, // chat_waiting
        0x828c73a35d6b1c0a, // chat_composer
        0x336344c70c851544, // chat_picked
        0x404446eb993dd555, // chat_scrolled
        0xa6d0dcccaae204f6, // chat_question
    ],
    // Dark at 2
    [
        0xd211836f5dcb7548, // chat_finding
        0x0c94100d5ab7460b, // chat_missing
        0xd11591ee80ab6397, // chat
        0xec5068206a45e5f8, // chat_waiting
        0xee8c28267b0af3c8, // chat_composer
        0x6a515ed6af7ed950, // chat_picked
        0xbbb09541105aaf77, // chat_scrolled
        0x503c5a806a45c7b1, // chat_question
    ],
    // Light at 1
    [
        0xcfe875ed39718011, // chat_finding
        0x8599e48c1a3a84f1, // chat_missing
        0xee98c0f4e9c9b8df, // chat
        0x8dd5c5644401e39e, // chat_waiting
        0xc9eb7c27910b0a3e, // chat_composer
        0xc4a1da2ac75e7e03, // chat_picked
        0xe23ae76ee4fa5bff, // chat_scrolled
        0xfbb6e5a61a1a9b5f, // chat_question
    ],
    // Light at 2
    [
        0x6a839f3b4ab2b79c, // chat_finding
        0x50c48f549721099b, // chat_missing
        0x9665dc98473271fb, // chat
        0x02aaf735fffad541, // chat_waiting
        0x57f4e1b82e7ef3e0, // chat_composer
        0xc1031423a1b0c52c, // chat_picked
        0x487f717d28fa8f37, // chat_scrolled
        0x7472c7255b9cfe88, // chat_question
    ],
];

#[test]
fn the_chats_paint_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_CHATS.iter().map(|row| row.as_slice()).collect();
    check(&CHATS, &golden, paint_chats);
}

/// With `GERDR_DUMP_DIR` set, write the chats there.
#[test]
fn dump_chats() {
    let Some(dir) = std::env::var_os("GERDR_DUMP_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let painter = Painter::new();
    for (theme, scale) in CASES {
        let tag = format!("{}_{}", theme.name(), scale);
        for (i, name) in CHATS.iter().enumerate() {
            let view = chat_view(i);
            let w = (view.rect.size.w as f64 * scale).round() as usize;
            let buffer = painter.agent_chat(&view, scale, theme.palette());
            dump(&dir, &format!("{name}_{tag}"), buffer, w);
        }
    }
}
