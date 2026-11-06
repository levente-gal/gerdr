//! Pixel-identity checks: every chrome surface painted from a fixed view,
//! at scale 1 and 2 in both themes, hashed. The hashes change only when
//! the drawing does; a change made on purpose records the new ones (run
//! the test and copy the table it prints).

use std::sync::atomic::{AtomicUsize, Ordering};
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
use crate::feature::Stage;
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
        rect: super::pane_bar_rect(pane, BarButtons::Terminal),
        grip: Rect::default(),
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
        lit: true,
        lit_level: 1.0,
        hover: Some(BarButton::Restart),
        visible: true,
        alpha: 1.0,
        slide: 0,
    }
}

fn menu() -> MenuView {
    MenuView {
        anchor: Rect::new((150, 0).into(), (28, 34).into()),
        rows: vec![MenuRow {
            label: "Open folder…".into(),
            hint: None,
            enabled: true,
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
    "pane_pill",
    "pane_pill_waiting",
    "pane_handle",
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
        hash(painter.pane_bar(&pane_pill(), scale, p)),
        hash(painter.pane_bar(&pane_pill_waiting(), scale, p)),
        hash(painter.pane_handle((600, super::PANE_GRIP).into(), true, scale, p)),
    ]
}

/// A web page's bar out as its pill, the copy button hovered.
fn pane_pill() -> PaneBarView {
    let pane = Rect::new((0, 0).into(), (900, 400).into());
    let buttons = BarButtons::Web { history: true };
    PaneBarView {
        rect: super::pane_bar_rect(pane, buttons),
        grip: super::pane_grip_rect(pane),
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

/// A stage agent's terminal, waiting for its prompt.
fn pane_pill_waiting() -> PaneBarView {
    let pane = Rect::new((0, 0).into(), (900, 400).into());
    let buttons = BarButtons::Waiting;
    PaneBarView {
        rect: super::pane_bar_rect(pane, buttons),
        grip: super::pane_grip_rect(pane),
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
        0xea617f04672206a4, // sidebar
        0xed1972252f36590d, // tab_strip
        0x666fd35c1955aaa6, // pane_bar
        0x866d59a70ebe252d, // help
        0x378d0ffdb2449b77, // menu
        0xfc09c643c2d57ae1, // switcher
        0xe4d55e0a322e116f, // welcome
        0x1fac2c9284ad07fa, // quit
        0x92a49c766cb37d1c, // project_menu
        0xaf72881e8b741219, // project_form
        0xbd0d423ca3de12bd, // toast
        0x1b80b690e340528a, // agent_strip
    ],
    // Dark at 2
    [
        0x017152f1d8605ca8, // sidebar
        0x75280e3af24f953f, // tab_strip
        0x0dc4d7db9b5967cd, // pane_bar
        0x56045b72862ed173, // help
        0xd724a365fa94d169, // menu
        0x1a39f33e0a29f83b, // switcher
        0xc6a2b9324f6b7b75, // welcome
        0x91306b9e268dc79d, // quit
        0x68b0b29c55156a50, // project_menu
        0x1d108a7b493c0cd1, // project_form
        0x8a9a152ce2a17651, // toast
        0x95f9334ab0c42dc0, // agent_strip
    ],
    // Light at 1
    [
        0xbd043f5a886c0db4, // sidebar
        0xf5c707c6dab41293, // tab_strip
        0xa514c9cbf0c81198, // pane_bar
        0x8c0302567b35f0c3, // help
        0xf82050c3b2848862, // menu
        0x422064c274f92b90, // switcher
        0x232afdce7d51101b, // welcome
        0xc9fc61e2fab1872e, // quit
        0xaf88a1744cdd63ab, // project_menu
        0xe2e62fecf8cb83c6, // project_form
        0x3023720e0af5627f, // toast
        0xa37db337f4fcec5f, // agent_strip
    ],
    // Light at 2
    [
        0xd949d7e1c0297a42, // sidebar
        0x65c73e85a9732c79, // tab_strip
        0x887475a089d81641, // pane_bar
        0x22f444d49649f855, // help
        0xc17d1e64d3c77f97, // menu
        0x77eb0acf1f4d4fd5, // switcher
        0xc5b0dff47ac46d40, // welcome
        0x0a88810aa7ad751e, // quit
        0xad9c7bc5aa554ff4, // project_menu
        0x527909807a17afb5, // project_form
        0xd1410a8fb372b973, // toast
        0x8aeabdb50084b1d6, // agent_strip
    ],
];

/// The recorded hashes of `OVERLAYS`, per case in `CASES`.
const GOLDEN_OVERLAYS: [[u64; 11]; 4] = [
    // Dark at 1
    [
        0x639ee0c7e0afc916, // full_menu
        0x833a9487b45a04b1, // help_pinned
        0xcb41240819a1115b, // toast_action
        0x6960fb479b40da3d, // tooltip
        0xd5b2438c44e2d360, // placeholder
        0x0b01f4540b304877, // placeholder_timed_out
        0x7a426c6be23d66a6, // spinner
        0x5f7da883de161583, // blink_dot
        0x693a761f12f44c57, // pane_pill
        0xa43d0c0dae9f2d24, // pane_pill_waiting
        0x14c1b5fef3447fe5, // pane_handle
    ],
    // Dark at 2
    [
        0x642484e32b14644f, // full_menu
        0xa52ac88120332539, // help_pinned
        0x73d72308b6d3d979, // toast_action
        0xad169f115b70c54c, // tooltip
        0x94bf628b2c8d8e61, // placeholder
        0x430bc916e7ac895b, // placeholder_timed_out
        0x2f28c94a7002e520, // spinner
        0x3938d7cb530659b5, // blink_dot
        0xeb8497a128455a3a, // pane_pill
        0xf8b494e5a75d5fcc, // pane_pill_waiting
        0x190a245af3371715, // pane_handle
    ],
    // Light at 1
    [
        0x33da81c85d2c4485, // full_menu
        0x50daf004ca030b63, // help_pinned
        0x7fe278dd9779e673, // toast_action
        0x46ec14c3018cfa90, // tooltip
        0x848913a059883552, // placeholder
        0xe7444f2e3fd03c3f, // placeholder_timed_out
        0x6cb91debc8d3c473, // spinner
        0x55c89cc3e0120adb, // blink_dot
        0x384a5c439f6867e7, // pane_pill
        0x17fed7d408d378a4, // pane_pill_waiting
        0x64ee76af43172b3d, // pane_handle
    ],
    // Light at 2
    [
        0x7d8b5365601ae144, // full_menu
        0x357c8c2ee4c38806, // help_pinned
        0xe5cceb11e9d0de1d, // toast_action
        0x647ba732cdd8bd58, // tooltip
        0x9bc50d6953da1fd5, // placeholder
        0x2a2964b70d0b1b41, // placeholder_timed_out
        0x5ad35f5c0797002e, // spinner
        0x349f0f6edf45454d, // blink_dot
        0x9f76f3470b72f011, // pane_pill
        0x48c010ad41a8a934, // pane_pill_waiting
        0x3973c1a54dd3decd, // pane_handle
    ],
];

/// Run `job` for every index below `count`, on every core, and give the
/// results in index order. A `Painter` is not `Sync`, so each thread makes
/// its own; the slow surfaces take most of the test run in a dev build.
pub(super) fn fan_out<T: Send>(count: usize, job: impl Fn(&Painter, usize) -> T + Sync) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut done: Vec<(usize, T)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..cores.min(count))
            .map(|_| {
                scope.spawn(|| {
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
        assert!(painter.font.is_some(), "the golden hashes need a font");
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
        assert!(painter.font.is_some(), "the golden hashes need a font");
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
        };
        dump(
            &dir,
            &format!("confirm_finish_{tag}"),
            painter.confirm_dialog(&finish, (800, 600).into(), scale, p),
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

/// A feature workspace's tab strip: its panel tab first and shown, with
/// no "×", then the usual tabs.
fn pinned_strip_view() -> TabStripView {
    let mut tabs = tabs();
    tabs.insert(
        0,
        TabView {
            title: "Feature".into(),
            pinned: true,
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
        0xf152f0daf2737043, // pinned_tab_strip
    ],
    // Dark at 2
    [
        0x58fb2e701d2fe9fd, // pinned_tab_strip
    ],
    // Light at 1
    [
        0xf2f7f19a81351ca3, // pinned_tab_strip
    ],
    // Light at 2
    [
        0xf7f40168b0293f90, // pinned_tab_strip
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
    ]
}

const STATUS: [&str; 2] = ["footer", "tasks_panel"];

/// The recorded hashes of `STATUS`, per case in `CASES`.
const GOLDEN_STATUS: [[u64; 2]; 4] = [
    // Dark at 1
    [
        0x3821a1cf1a7fe182, // footer
        0x7b7eaff2a33d9052, // tasks_panel
    ],
    // Dark at 2
    [
        0x910ba2519163ffe3, // footer
        0x53a939dcd1f191bc, // tasks_panel
    ],
    // Light at 1
    [
        0x8a8087fd34aec460, // footer
        0x4ccf9615670317c4, // tasks_panel
    ],
    // Light at 2
    [
        0x28f7c3f0d800d54e, // footer
        0x2864d1396204ac44, // tasks_panel
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
        let panel = super::tasks_layout(&status_panel(), STATUS_OUTPUT.into(), footer).panel;
        dump(
            &dir,
            &format!("tasks_panel_{tag}"),
            painter.tasks_panel(&status_panel(), STATUS_OUTPUT.into(), footer, scale, p),
            px(panel.size.w + 2 * super::SHADOW),
        );
    }
}

/// The new feature dialog over three repositories, the second unticked,
/// and a linked project's one, the keyboard on the title.
fn new_feature() -> super::NewFeatureView {
    let repo = |name: &str, base: &str, touched| super::NewFeatureRepo {
        name: name.into(),
        place: format!(
            "from {base} \u{b7} {}",
            if touched { "linked" } else { "left out" }
        ),
        touched,
        fixed: false,
    };
    super::NewFeatureView {
        heading: "New feature in \u{201c}aft\u{201d}".into(),
        title: Line::from("Bulk export of invoices"),
        focus: None,
        root: "~/Work/aft".into(),
        root_failed: false,
        projects: "Projects: aft (home) \u{b7} billing".into(),
        link: super::LINK_BUTTON[0],
        repos: vec![
            repo("api", "dev", true),
            repo("shared", "main", false),
            repo("web", "dev", true),
            repo("billing: ledger", "main", false),
        ],
        top: 0,
        error: None,
        hover: Some(super::NewFeatureHit::Button(1)),
        keys: vec![
            ("Tab", "next"),
            ("Space", "tick"),
            ("Ctrl+Shift+L", "link project"),
            ("Esc", "clear"),
        ],
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
        hash(painter.sidebar_view(&features_sidebar(), (SIDEBAR_W, 520).into(), scale, p)),
        hash(painter.plan_sheet(&plan_sheet(false), size, scale, p)),
        hash(painter.plan_sheet(&plan_sheet(true), size, scale, p)),
        hash(painter.plan_sheet(&plan_sheet_narrow(), PLAN_NARROW.into(), scale, p)),
        hash(painter.plan_sheet(&plan_sheet_own(), size, scale, p)),
        hash(painter.plan_sheet(&plan_sheet_keys(), PLAN_NARROW.into(), scale, p)),
        hash(painter.plan_sheet(&plan_sheet_empty(), size, scale, p)),
        hash(painter.sidebar_view(
            &feature_workspace_sidebar(),
            (SIDEBAR_W, 520).into(),
            scale,
            p,
        )),
        hash(painter.sidebar_view(&needs_you_sidebar(), (SIDEBAR_W, 520).into(), scale, p)),
        hash(painter.sidebar_view(&no_features_sidebar(), (SIDEBAR_W, 520).into(), scale, p)),
    ]
}

/// A small output, for the plan review's sheet to fit.
const PLAN_NARROW: (i32, i32) = (480, 420);

/// The plan review's sheet: a suggestion of each state and kind, one of the
/// user's own, one left out, long text, a citation not found; `note`: a note
/// being written, else the last refusal said.
fn plan_sheet(note: bool) -> super::PlanSheetView {
    use super::{PlanCitation, PlanDetail, PlanField, PlanMark, PlanRow};
    use crate::feature::{plan_review::Kind, review_import::Severity};
    let row = |id: &str, mark, severity, kind, title: &str, plan: &str| PlanRow {
        id: id.into(),
        mark,
        severity,
        kind,
        title: title.into(),
        plan: plan.into(),
        yours: false,
        dropped: false,
        note: String::new(),
    };
    let rows = vec![
        row(
            "r1-1",
            PlanMark::Accepted,
            Severity::High,
            Kind::Gap,
            "Exports aren't limited per user",
            "Milestone 2",
        ),
        row(
            "r1-2",
            PlanMark::Open,
            Severity::High,
            Kind::Risk,
            "The retry loop has no cap, so a server that stays down keeps a worker busy for ever",
            "Milestone 3: the export worker and its queue",
        ),
        row(
            "r1-3",
            PlanMark::Rejected,
            Severity::Low,
            Kind::Simplify,
            "The CSV writer could be the one reports use",
            "Milestone 1",
        ),
        row(
            "r1-4",
            PlanMark::Open,
            Severity::Medium,
            Kind::EdgeCase,
            "An invoice cancelled during the export",
            "Milestone 3",
        ),
        row(
            "r1-5",
            PlanMark::Open,
            Severity::Medium,
            Kind::Question,
            "Should a cancelled invoice count in the total?",
            "Milestone 4",
        ),
        PlanRow {
            dropped: true,
            ..row(
                "r1-6",
                PlanMark::Accepted,
                Severity::Low,
                Kind::Gap,
                "The empty state has no words",
                "Milestone 5",
            )
        },
        PlanRow {
            yours: true,
            ..row(
                "u1",
                PlanMark::Accepted,
                Severity::Medium,
                Kind::Gap,
                "Keep the old export for a release",
                "",
            )
        },
        row(
            "r1-7",
            PlanMark::Open,
            Severity::Low,
            Kind::Gap,
            "Past the fold",
            "Milestone 6",
        ),
    ];
    let hints: &[(&str, &str)] = if note {
        &[("Enter", "save"), ("Esc", "save")]
    } else {
        &[
            ("Space", "accept"),
            ("r", "reject"),
            ("n", "note\u{2026}"),
            ("a", "add\u{2026}"),
            ("Del", "clear"),
            ("Ctrl+Enter", "final plan"),
            ("Ctrl+Shift+Enter", "ask"),
            ("F1", "keys"),
            ("Esc", "close"),
        ]
    };
    super::PlanSheetView {
        title: "Plan review of \u{201c}Bulk export of invoices\u{201d}".into(),
        sub: "4 to decide \u{b7} 3 accepted \u{b7} 1 rejected".into(),
        missing: Some("1 citation not found".into()),
        rows,
        cursor: 1,
        top: 0,
        detail: Some(PlanDetail {
            title: "The retry loop has no cap, so a server that stays down keeps a worker busy for ever".into(),
            plan: "Milestone 3: the export worker and its queue".into(),
            dropped: false,
            question: false,
            why: "When the mail server is down, every export retries at once and goes on doing so: \
                  the queue fills, and nobody else's export starts until the server is back. \
                  With 500 invoices that is hours of a busy worker for nothing."
                .into(),
            change: "Say in Milestone 3 that a send is tried at most five times, with a pause that doubles, and that it then fails with a message the user sees."
                .into(),
            evidence: vec![
                PlanCitation {
                    place: "api: src/export/worker.rs:88".into(),
                    quote: "loop { if send(&mail).is_ok() { break; } }".into(),
                    not_found: false,
                },
                PlanCitation {
                    place: "api: src/export/queue.rs:12".into(),
                    quote: "const MAX_RETRIES: u32 = 5;".into(),
                    not_found: true,
                },
            ],
            note: "Five is fine, but not for the nightly run".into(),
        }),
        // Down to the citations, the note's sheet from the start.
        detail_top: if note { 0 } else { 6 },
        editing: note.then(|| PlanField {
            label: "Your note on this suggestion",
            placeholder: "What you say of it",
            line: Line::from("Five is fine, but not for the nightly run"),
        }),
        armed: None,
        notice: (!note).then(|| "4 suggestions still to decide".into()),
        hover: (!note).then_some(super::PlanHit::Row(3)),
        finish: "Write final plan",
        primary: None,
        keys: None,
        keys_top: 0,
        hints: hints
            .iter()
            .map(|(k, w)| ((*k).to_owned(), (*w).to_owned()))
            .collect(),
    }
}

/// The sheet on a small output, the cursor's row scrolled into view.
fn plan_sheet_narrow() -> super::PlanSheetView {
    let mut view = plan_sheet(false);
    view.top = super::plan_sheet_reveal(&view, PLAN_NARROW.into());
    view
}

/// The sheet on one of the user's own, Del pressed on it once, down where
/// the one left out by the reviewer shows too.
fn plan_sheet_own() -> super::PlanSheetView {
    let mut view = plan_sheet(false);
    view.cursor = 6;
    view.top = super::plan_sheet_reveal(&view, FEATURE_OUTPUT.into());
    view.armed = Some(6);
    view.notice = None;
    view.hover = None;
    view.detail = Some(super::PlanDetail {
        title: "Keep the old export for a release".into(),
        plan: String::new(),
        dropped: false,
        question: false,
        why: String::new(),
        change: String::new(),
        evidence: Vec::new(),
        note: String::new(),
    });
    view.detail_top = 0;
    view.primary = Some(super::PLAN_FINISH);
    view.hints = vec![
        ("Del".to_owned(), "remove".to_owned()),
        ("Esc".to_owned(), "keep it".to_owned()),
    ];
    view
}

/// The keys list on a small output, past what fits.
fn plan_sheet_keys() -> super::PlanSheetView {
    let keys = [
        ("Up / Down", "move between the suggestions"),
        ("Ctrl+J / Ctrl+K", "move between the suggestions"),
        ("Home / End", "go to the first or last suggestion"),
        ("PgUp / PgDn", "scroll what the suggestion says"),
        (
            "Space",
            "accept it, or clear that; a question wants its answer",
        ),
        ("r", "reject it, or clear that"),
        ("n", "write a note on it"),
        ("a", "add a suggestion of your own"),
        ("Del", "clear its decision, or remove one of your own"),
    ];
    let mut view = plan_sheet(false);
    view.notice = None;
    view.hover = None;
    view.keys = Some(
        keys.iter()
            .map(|(k, w)| ((*k).to_owned(), (*w).to_owned()))
            .collect(),
    );
    view.hints = vec![("Esc".to_owned(), "back".to_owned())];
    view
}

/// The sheet of a review that found nothing.
fn plan_sheet_empty() -> super::PlanSheetView {
    let mut view = plan_sheet(false);
    view.sub = "no suggestions".into();
    view.missing = None;
    view.rows = Vec::new();
    view.cursor = 0;
    view.detail = None;
    view.notice = None;
    view.hover = None;
    view.finish = "Use the plan as it is";
    view.primary = Some(super::PLAN_FINISH);
    view.hints = [
        ("a", "add\u{2026}"),
        ("Ctrl+Enter", "use the plan"),
        ("F1", "keys"),
        ("Esc", "close"),
    ]
    .iter()
    .map(|(k, w)| ((*k).to_owned(), (*w).to_owned()))
    .collect();
    view
}

const FEATURES: [&str; 11] = [
    "new_feature",
    "sidebar_features",
    "plan_sheet",
    "plan_sheet_note",
    "plan_sheet_narrow",
    "plan_sheet_own",
    "plan_sheet_keys",
    "plan_sheet_empty",
    "sidebar_feature_workspace",
    "sidebar_needs_you",
    "sidebar_no_features",
];

/// The recorded hashes of `FEATURES`, per case in `CASES`.
const GOLDEN_FEATURES: [[u64; 11]; 4] = [
    // Dark at 1
    [
        0x73e88729cb0c3112, // new_feature
        0xa77cf538d219f4ef, // sidebar_features
        0xce73d235eada39de, // plan_sheet
        0xf0ef6e28af47b05c, // plan_sheet_note
        0x444a60ab9e0f4abf, // plan_sheet_narrow
        0x86b4df03a4af9916, // plan_sheet_own
        0xe472df71a44ddf16, // plan_sheet_keys
        0x93dd44d0af75ae63, // plan_sheet_empty
        0x4f489a294fd6cfe9, // sidebar_feature_workspace
        0x42f966c250e9899d, // sidebar_needs_you
        0x0cbbecc8e276c3fc, // sidebar_no_features
    ],
    // Dark at 2
    [
        0xbc9770d9310bd319, // new_feature
        0x21105ece42c0dd0e, // sidebar_features
        0x354b777d647b74ae, // plan_sheet
        0x49003f081deb3c2e, // plan_sheet_note
        0x77de8b18241bb4b7, // plan_sheet_narrow
        0x645f1307207f69db, // plan_sheet_own
        0x6ad99a47f32750f1, // plan_sheet_keys
        0xf4f48a436676b8ed, // plan_sheet_empty
        0x25a6c3532952ab77, // sidebar_feature_workspace
        0x7f5b07bc5817daa7, // sidebar_needs_you
        0xf97a94d1cbc468b6, // sidebar_no_features
    ],
    // Light at 1
    [
        0xb1780dfd60b1e9a8, // new_feature
        0xaf41feb3e3003f04, // sidebar_features
        0x6da7e766b94ea319, // plan_sheet
        0x6900962e338f6172, // plan_sheet_note
        0x5d1256e75161f348, // plan_sheet_narrow
        0xa3d45cd4852074e7, // plan_sheet_own
        0x60c2fae25b1a30c4, // plan_sheet_keys
        0x6b284b7ef20e69aa, // plan_sheet_empty
        0x8c099eb3525e36fa, // sidebar_feature_workspace
        0x0f94ad20e6efe803, // sidebar_needs_you
        0xb97e2bad7455858d, // sidebar_no_features
    ],
    // Light at 2
    [
        0x757537a93b40f9fa, // new_feature
        0x690bf93cbdab10ee, // sidebar_features
        0x5a9fa2f0a7a81443, // plan_sheet
        0x8bc60f688ce6245b, // plan_sheet_note
        0xf99dc433f41c1492, // plan_sheet_narrow
        0x676f86f8aa7b2284, // plan_sheet_own
        0x64823c62e6db84ec, // plan_sheet_keys
        0x13fb250de721df9f, // plan_sheet_empty
        0x3653c34b3b5ea64a, // sidebar_feature_workspace
        0xe73df09db1908db1, // sidebar_needs_you
        0x7ae57ba63db05473, // sidebar_no_features
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
        let dialog = super::new_feature_layout(size, 3, false).panel;
        dump(
            &dir,
            &format!("new_feature_{tag}"),
            painter.new_feature_dialog(&new_feature(), size, scale, p),
            px(dialog.size.w + shadow),
        );
        for (name, view, output) in [
            ("plan_sheet", plan_sheet(false), FEATURE_OUTPUT),
            ("plan_sheet_note", plan_sheet(true), FEATURE_OUTPUT),
            ("plan_sheet_narrow", plan_sheet_narrow(), PLAN_NARROW),
            ("plan_sheet_own", plan_sheet_own(), FEATURE_OUTPUT),
            ("plan_sheet_keys", plan_sheet_keys(), PLAN_NARROW),
            ("plan_sheet_empty", plan_sheet_empty(), FEATURE_OUTPUT),
        ] {
            let panel = super::plan_sheet_layout(&view, output.into()).panel;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.plan_sheet(&view, output.into(), scale, p),
                px(panel.size.w + shadow),
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
        0xfe9670c1db2227bb, // identity
        0xef59994443850f38, // identity_error
    ],
    // Dark at 2
    [
        0x9cace75ad1c239a3, // identity
        0x8d7402004f72d9cb, // identity_error
    ],
    // Light at 1
    [
        0x2dcb64793711569e, // identity
        0xc52c2f5d94c6d25a, // identity_error
    ],
    // Light at 2
    [
        0xd8b061bf59d82bc3, // identity
        0x09a2ede92adb49a9, // identity_error
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
    [
        0xb84e55277b561625, // restart
        0x55e74402e1d07d07, // restart_sandbox
    ],
    [
        0xf211e65cd2b4b59f, // restart
        0x28f6476fac5dce8d, // restart_sandbox
    ],
    [
        0x76ed5d9183dab2ce, // restart
        0xc71a8cacc89f09c8, // restart_sandbox
    ],
    [
        0x4d6f974e84ed3255, // restart
        0x9f6f2f448431032e, // restart_sandbox
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

const COMMANDS: [&str; 6] = [
    "commands",
    "commands_setup",
    "commands_stale",
    "commands_low",
    "commands_run",
    "commands_feature",
];

/// The recorded hashes of `COMMANDS`, per case in `CASES`.
const GOLDEN_COMMANDS: [[u64; 6]; 4] = [
    // Dark at 1
    [
        0xff031e5e6db3a64f, // commands
        0x3fe63a0dbef9f070, // commands_setup
        0x2aab5777643adee7, // commands_stale
        0x9a123ac89261edc7, // commands_low
        0x8f88e245d96bbcbc, // commands_run
        0x5510179c6435358b, // commands_feature
    ],
    // Dark at 2
    [
        0xd5968b6e9a017058, // commands
        0xcddedce03bc25a32, // commands_setup
        0x4c08fe9429e83f07, // commands_stale
        0x62f7f21243aa44c0, // commands_low
        0x115701e19f38427d, // commands_run
        0x9b521764fde307fe, // commands_feature
    ],
    // Light at 1
    [
        0x2164a43b0806ac61, // commands
        0x46e153a554416741, // commands_setup
        0x36e0025fe130b86d, // commands_stale
        0x5c2304a89984beda, // commands_low
        0xb45b5b87e9f4cc59, // commands_run
        0xabb49a31021a1a6c, // commands_feature
    ],
    // Light at 2
    [
        0xdf8703a23c5b1362, // commands
        0xfad4cbca3289ece2, // commands_setup
        0x05613c1913910cd0, // commands_stale
        0xc13b51a13f012fdc, // commands_low
        0x4853624b12a58330, // commands_run
        0xf4cb147dcf114360, // commands_feature
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
                3 => "running 14m".into(),
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
    let repo = |name: &str,
                line: &str,
                touched: bool,
                button: Option<&'static str>,
                stat: Option<(usize, usize)>| FeatureRepoRow {
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
        ("Tab", "repositories"),
        ("Enter", "mark done"),
        ("Ctrl+O", "open tab"),
        ("Ctrl+C", "copy path"),
        ("Ctrl+N", "new"),
        ("Del", "abandon"),
        ("F1", "all keys"),
    ];
    FeatureTabView {
        title: "Bulk export of invoices".into(),
        sub: "bulk-export \u{b7} ~/Work/aft/.features/bulk-export \u{b7} started 3h ago".into(),
        which: Some((0, 2)),
        remove: "Abandon\u{2026}",
        rail,
        back: None,
        repos: vec![
            repo("api", "feature/bulk-export", true, None, Some((212, 38))),
            repo("web", "feature/bulk-export", true, None, Some((96, 4))),
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
                },
                PageSection::Milestones {
                    label: "Build".into(),
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
                },
                PageSection::Milestones {
                    label: "Review 1".into(),
                    done: 1,
                    total: 2,
                    items: [
                        (true, "r1-1: Export drops the last row"),
                        (false, "r1-2: Long jobs lose their progress"),
                    ]
                    .map(|(d, t)| (d, t.to_owned()))
                    .into(),
                },
            ],
        },
        activity: activity.into(),
        scroll: [0.0; 3],
        hover: None,
        hints: hints.map(|(k, a)| (k.to_owned(), a.to_owned())).into(),
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
        title: "Plan".into(),
        about: Stage::Plan.about().into(),
        status: Some(tab_line("plan.md is ready for review", CheckTone::Blocker)),
        buttons: vec![
            button("Review", ButtonStyle::Primary),
            button("Show agent", ButtonStyle::Plain),
        ],
        sections: vec![
            PageSection::Agent {
                icon: AgentIcon::Codex,
                name: "codex".into(),
                dot: Dot::Idle,
                status: "waiting for you \u{b7} tab 2".into(),
                tail: Vec::new(),
                buttons: vec![1],
                picks: Vec::new(),
                model: None,
            },
            PageSection::File {
                name: "plan.md".into(),
                state: tab_line("ready for review", CheckTone::Ready),
                buttons: vec![0],
            },
        ],
    };
    view
}

/// A brief as a file's page shows it, every kind of block in it.
fn doc_blocks() -> Vec<crate::md_blocks::DocBlock> {
    use crate::md_blocks::{DocBlock, ItemMark};
    let item = |depth, mark, text: &str| DocBlock::Item {
        depth,
        mark,
        text: text.into(),
    };
    vec![
        DocBlock::Heading {
            level: 1,
            text: "Log in with a password".into(),
        },
        DocBlock::Para(
            "People sign in with an email and a password, and stay signed in. A wrong \
             password says so, without saying which of the two it was."
                .into(),
        ),
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
        ],
        sections: vec![
            PageSection::Agent {
                icon: AgentIcon::Claude,
                name: "claude".into(),
                dot: Dot::Idle,
                status: "waiting for you \u{b7} tab 2".into(),
                tail: Vec::new(),
                buttons: vec![1],
                picks: Vec::new(),
                model: None,
            },
            PageSection::File {
                name: "brief.md".into(),
                state: tab_line("ready for review", CheckTone::Ready),
                buttons: vec![0],
            },
            PageSection::Doc {
                name: "brief.md".into(),
                blocks: std::sync::Arc::new(doc_blocks()),
            },
        ],
    };
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
            icon: AgentIcon::Claude,
            name: "claude".into(),
            dot: Dot::Done,
            status: "started 2h ago".into(),
            tail: Vec::new(),
            buttons: vec![1],
            picks: Vec::new(),
            model: None,
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
            state: tab_line("Verify failures \u{b7} fix plan ready", CheckTone::Ready),
            buttons: vec![0, 1, 2],
        },
    );
    view
}

/// Build's page after Verify's failures came back to it twice, and a setup.
fn feature_tab_verify_fixes() -> super::FeatureTabView {
    use super::{CheckTone, PageSection};
    let mut view = feature_tab();
    view.page.sections.insert(
        1,
        PageSection::Lines {
            label: "Verify failures".into(),
            lines: vec![
                tab_line(
                    "Review 2 \u{b7} run 3 failed 12m ago \u{b7} Claude Code \u{b7} making fixes \u{b7} 1 of 3 made",
                    CheckTone::Blocker,
                ),
                tab_line("api: `cargo test export::` exited with 101", CheckTone::Blocker),
                tab_line(
                    "error[E0599]: no method named `stream` found for struct `Export` in the current scope, which is long enough to reach the side of the page at the narrow width",
                    CheckTone::Note,
                ),
                tab_line("web: `npm run lint` exited with 2", CheckTone::Blocker),
                tab_line(
                    "Review 1 \u{b7} run 2 failed 2h ago \u{b7} Cursor \u{b7} fixes made \u{b7} run 3 failed",
                    CheckTone::Note,
                ),
            ],
        },
    );
    view.page.sections.push(PageSection::Lines {
        label: "Setup".into(),
        lines: vec![
            tab_line("copied api: .env", CheckTone::Note),
            tab_line("ran web: npm ci", CheckTone::Note),
        ],
    });
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
        title: "Ship".into(),
        about: Stage::Ship.about().into(),
        status: None,
        buttons: vec![PageButton {
            label: "Ship\u{2026}".into(),
            style: ButtonStyle::Off,
        }],
        sections: vec![
            PageSection::Lines {
                label: "Ship check".into(),
                lines: vec![
                    tab_line("api has changes not committed", CheckTone::Blocker),
                    tab_line("web is 3 commits behind dev", CheckTone::Blocker),
                ],
            },
            PageSection::ShipGroup(ship_group(
                "api \u{b7} 4 commits \u{b7} +120 \u{2212}8 \u{b7} into main",
                1,
                0,
                None,
                vec![
                    ship_row(
                        1,
                        "Merge into main in ~/code/api",
                        Some(("api's main worktree is on dev: switch it to main", true)),
                        true,
                    ),
                    ship_row(2, "Push main to origin", None, false),
                ],
            )),
            PageSection::ShipGroup(ship_afterwards(3, false)),
        ],
    };
    view
}

/// A group of the Ship page: `row` its choice's row, `on` its way, if any.
fn ship_group(
    head: &str,
    on: usize,
    row: usize,
    note: Option<&str>,
    steps: Vec<super::ShipStepRow>,
) -> super::ShipGroup {
    super::ShipGroup {
        head: head.into(),
        caption: false,
        route: Some(super::ShipRouteRow { row, on }),
        note: note.map(str::to_owned),
        steps,
        selected: None,
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
            ship_row(
                4,
                "Merge into dev in ~/code/web",
                Some(("Squash; you edit the message in a tab", false)),
                true,
            ),
            ship_row(5, "Push dev to origin", None, true),
        ],
    );
    web.selected = Some(3);
    let mut docs = ship_group(
        "docs \u{b7} 1 commit \u{b7} +4 \u{2212}0 \u{b7} into main",
        2,
        6,
        Some("Nothing gets pushed or merged"),
        Vec::new(),
    );
    docs.selected = Some(3);
    view.page = StagePage {
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
            PageSection::ShipGroup(ship_afterwards(7, false)),
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
            },
        ],
    };
    let mut hints = vec![
        ("Left / Right", "stage"),
        ("Enter", "go to the current stage"),
    ];
    if !locked {
        hints.push(("Ctrl+Shift+Enter", "reopen"));
    }
    hints.extend([
        ("Tab", "repositories"),
        ("Ctrl+O", "open tab"),
        ("Ctrl+C", "copy path"),
        ("F1", "all keys"),
    ]);
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
                icon: crate::agent_icons::AgentIcon::Claude,
                name: "claude".into(),
                dot: super::Dot::Idle,
                status: "waiting for you \u{b7} tab 2".into(),
                tail: Vec::new(),
                buttons: vec![1],
                picks: Vec::new(),
                model: None,
            },
            PageSection::File {
                name: "plan.md".into(),
                state: tab_line("ready for review", CheckTone::Ready),
                buttons: vec![0],
            },
            PageSection::Doc {
                name: "plan.md".into(),
                blocks: std::sync::Arc::new(doc_blocks()),
            },
        ],
    };
    let hints = [
        ("Left / Right", "stage"),
        ("Enter", "review plan"),
        ("Tab", "repositories"),
        ("Ctrl+O", "open tab"),
        ("Ctrl+C", "copy path"),
        ("F1", "all keys"),
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
    view.remove = "Remove worktrees";
    view.page = StagePage {
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

/// Build current, a later stage's page picked: no buttons.
fn feature_tab_later() -> super::FeatureTabView {
    use super::{CheckTone, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    select_node(&mut view, 6);
    view.hover = Some(super::FeatureTabHit::Node(7));
    view.page = StagePage {
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
        title: "AI review".into(),
        about: Stage::AiReview.about().into(),
        status: Some(tab_line("Comes after Verify", CheckTone::Note)),
        buttons: Vec::new(),
        sections: vec![PageSection::Agent {
            icon: AgentIcon::Generic,
            name: String::new(),
            dot: Dot::Unknown,
            status: String::new(),
            tail: Vec::new(),
            buttons: Vec::new(),
            picks: agent_chips(1, true),
            model: None,
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
    view.hover = Some(super::FeatureTabHit::AgentPick(0));
    view
}

/// `view` with the agent card's model button saying `label`, the pointer on it.
fn with_model_button(mut view: super::FeatureTabView, label: &str) -> super::FeatureTabView {
    for section in &mut view.page.sections {
        if let super::PageSection::Agent { model, .. } = section {
            *model = Some(label.into());
        }
    }
    view.hover = Some(super::FeatureTabHit::AgentModel);
    view
}

/// Build set to a fresh agent per milestone, the pointer on the other choice.
fn feature_tab_milestone_mode() -> super::FeatureTabView {
    use super::PageSection;
    let mut view = feature_tab();
    let after = (view.page.sections.iter())
        .position(|s| matches!(s, PageSection::Agent { .. }))
        .map_or(0, |i| i + 1);
    view.page
        .sections
        .insert(after, PageSection::MilestoneMode { fresh: true });
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
        title: "Plan".into(),
        about: Stage::Plan.about().into(),
        status: Some(tab_line("not written yet", CheckTone::Note)),
        buttons: vec![
            button("Send prompt", ButtonStyle::Primary),
            button("Show agent", ButtonStyle::Plain),
        ],
        sections: vec![PageSection::Agent {
            icon: AgentIcon::from_agent("Claude Code"),
            name: "Claude Code".into(),
            dot: Dot::Unknown,
            status: "waiting for you \u{b7} send the prompt".into(),
            tail: Vec::new(),
            buttons: vec![0, 1],
            picks,
            model: None,
        }],
    };
    view
}

/// On AI review with the code changed since: the last review to look at,
/// and the earlier ones under it, the oldest shown.
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
            made: state == "fix made",
            faded,
            collapsed: false,
            stripe: None,
            field: None,
            field_hint: "",
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
        title: "AI review".into(),
        about: Stage::AiReview.about().into(),
        status: Some(tab_line("Comes after Verify", CheckTone::Note)),
        buttons: vec![
            button("Show in diff", ButtonStyle::Quiet),
            button("Show", ButtonStyle::Plain),
            button("Hide", ButtonStyle::Plain),
        ],
        sections: vec![
            PageSection::Lines {
                label: "Findings \u{b7} 1 sent to Build \u{b7} 1 fix made".into(),
                lines: vec![tab_line(
                    "From the last review. The code has changed since.",
                    CheckTone::Note,
                )],
            },
            card(
                FindingTag::High,
                "Retry stops one short",
                "sent to Build",
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
            round("Review 2", "2 fixed", 1),
            round(
                "Review 1",
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
    at_stage(&mut view, 3, NodeMark::Current, "running 2m");
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
    view.rail[3].line = "running again".into();
    turned_back(&mut view, 5);
    view.page = StagePage {
        title: "AI review".into(),
        about: Stage::AiReview.about().into(),
        status: None,
        buttons: vec![PageButton {
            label: "Show agent".into(),
            style: ButtonStyle::Plain,
        }],
        sections: vec![
            PageSection::Agent {
                icon: AgentIcon::Claude,
                name: "claude".into(),
                dot: Dot::Working,
                status: "Build agent \u{b7} running again".into(),
                tail: Vec::new(),
                buttons: vec![0],
                picks: Vec::new(),
                model: None,
            },
            super::PageSection::Lines {
                label: "Findings".into(),
                lines: vec![tab_line("2 sent to fix", CheckTone::Note)],
            },
        ],
    };
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
        title: "Plan".into(),
        about: Stage::Plan.about().into(),
        status: Some(tab_line("not written yet", CheckTone::Note)),
        buttons: Vec::new(),
        sections: vec![PageSection::Agent {
            icon: AgentIcon::T3,
            name: "T3 Code".into(),
            dot: Dot::Unknown,
            status: "draft in T3 Code \u{b7} pick a model and send it".into(),
            tail: Vec::new(),
            buttons: Vec::new(),
            picks,
            model: None,
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
    view.repos[2].stat = Some((7, 0));
    at_stage(&mut view, 6, super::NodeMark::Current, "3 of 9 viewed");
    view
}

/// Manual test's page: a criterion of each kind, one long enough to wrap, a
/// failure sent to Build; `note`: a failure's note being written on its card.
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
    let mut buttons = vec![
        button("Plan fixes", ButtonStyle::Primary),
        button("Mark passed", ButtonStyle::Plain),
        button("Start sandbox\u{2026}", ButtonStyle::Plain),
    ];
    let cards: [(&[&str], FindingTag, [&str; 4]); 4] = [
        (
            &["Clear", "Fail\u{2026}"],
            FindingTag::Passed,
            [
                "Given two invoices, when I export both, then one CSV holds both",
                "",
                "AC-1",
                "",
            ],
        ),
        (
            &["Clear", "Edit note\u{2026}", "Pass"],
            FindingTag::Failed,
            [
                "Given no invoices, when I export, then the button is off",
                "sent to Build",
                "AC-2",
                "What went wrong: The button stays on and exports an empty file",
            ],
        ),
        (
            &["Clear", "Edit note\u{2026}", "Pass"],
            FindingTag::Failed,
            [
                "Given a cancelled invoice, when I export, then it is left out of the file and \
                 of the total at the bottom of the page, and the count above the table goes \
                 down by one",
                "",
                "AC-3",
                "What went wrong: It shows with a total of 0",
            ],
        ),
        (
            &["Pass", "Fail\u{2026}"],
            FindingTag::NotRun,
            [
                "Given 500 invoices, when I export, then it is done in a second",
                "",
                "AC-4",
                "",
            ],
        ),
    ];
    let mut sections = vec![PageSection::Lines {
        label: "Criteria".into(),
        lines: vec![tab_line("uncommitted changes in api", CheckTone::Note)],
    }];
    for (i, (labels, tag, [title, state, place, said])) in cards.into_iter().enumerate() {
        let at = buttons.len();
        buttons.extend(labels.iter().map(|l| button(l, ButtonStyle::Plain)));
        let writing = note && i == 3;
        sections.push(PageSection::Finding(FindingCard {
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
            made: false,
            faded: tag == FindingTag::Passed,
            collapsed: false,
            stripe: None,
            field: writing.then(|| Line::from("Takes 4 seconds")),
            field_hint: "",
            selected: i == if note { 3 } else { 2 },
            buttons: (at..at + labels.len()).collect(),
        }));
    }
    view.page = StagePage {
        title: Stage::ManualTest.label().into(),
        about: Stage::ManualTest.about().into(),
        status: Some(tab_line(
            "1 of 4 criteria passed \u{b7} 2 failed",
            CheckTone::Note,
        )),
        buttons,
        sections,
    };
    if note {
        view.hints = [("Enter", "save"), ("Esc", "save")]
            .iter()
            .map(|(k, w)| ((*k).to_owned(), (*w).to_owned()))
            .collect();
    }
    view
}

/// The feature tab's fixtures, by name.
fn feature_tabs() -> [(&'static str, super::FeatureTabView); 37] {
    [
        ("tab_build", feature_tab()),
        ("tab_verify_fixes", feature_tab_verify_fixes()),
        ("tab_plan_ready", feature_tab_plan_ready()),
        ("tab_verify_failed", feature_tab_verify_failed()),
        ("tab_verify_running", feature_tab_verify_running()),
        ("tab_fix_plan", feature_tab_fix_plan()),
        ("tab_ship_blocked", feature_tab_ship_blocked()),
        ("tab_ship_routes", feature_tab_ship_routes()),
        ("tab_ship_finish", feature_tab_ship_finish()),
        ("tab_done", feature_tab_done()),
        ("tab_later", feature_tab_later()),
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
        ("tab_plan_long", feature_tab_plan_long()),
        ("tab_manual_test", feature_tab_manual_test(false)),
        ("tab_manual_note", feature_tab_manual_test(true)),
        ("tab_review_rounds", feature_tab_review_rounds()),
    ]
}

/// Each fixture at the width it shows best at, and Build at all three.
const TAB_SURFACES: [(&str, i32); 47] = [
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
];

const FEATURE_TAB: [&str; 47] = [
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
];

/// The recorded hashes of `FEATURE_TAB`, per case in `CASES`.
const GOLDEN_FEATURE_TAB: [[u64; 47]; 4] = [
    // Dark at 1
    [
        0x6283066ad99ffd8d, // tab_build_860
        0xaa4a9c59e083ffc5, // tab_build_1180
        0x7cd5d2c2cd59dd02, // tab_build_1920
        0xc1278577e56967a2, // tab_plan_ready_1180
        0xe6c0a8c26922ed70, // tab_verify_failed_1920
        0xceae385ff36ea8be, // tab_verify_running_1180
        0x17565c26a855e9ae, // tab_fix_plan_860
        0x6de8a0eac93ce4f5, // tab_ship_blocked_1180
        0x2edecb062d49117f, // tab_ship_routes_1180
        0x3f5339a12a61cb80, // tab_ship_routes_860
        0x02f7b20f388ca2bc, // tab_ship_finish_1180
        0x5f710b55ff8ddf37, // tab_done_1920
        0x79859ce0110dd014, // tab_later_860
        0x9803b1f7c638747e, // tab_repos_1180
        0x9ac66eb68ad4356c, // tab_unfolded_860
        0xf5f4e82cc9622bf1, // tab_unfolded_1920
        0x1bd1244a05d242d2, // tab_shipped_1180
        0x960942ef0e94b650, // tab_repos_failed_1180
        0x6c466214009e25d4, // tab_repos_error_860
        0x7c29d720de32d5c5, // tab_more_repos_850
        0xf59168aaae7f823a, // tab_more_repos_mid_850
        0x6ef8dc4b53ce29c4, // tab_rail_wrap_500
        0x6d33a9a128a79826, // tab_rail_wrap_1180
        0x7f35f7fe16d2bf96, // tab_picks_later_1180
        0x713b9ddfe81db9fd, // tab_picks_running_860
        0x794d6c93750ad5ff, // tab_model_later_1180
        0x58c613f9c558daac, // tab_model_long_860
        0x3836b05b3315ee81, // tab_t3_draft_860
        0x130d12456bb8febf, // tab_milestone_mode_1180
        0x0e87f6bec78c8ca8, // tab_milestone_mode_860
        0x5ce4ec148de5244e, // tab_held_860
        0xb769e6bd04ceafe5, // tab_resume_1180
        0x3f752b44da659f99, // tab_resume_860
        0x90cad9c6a1fa38df, // tab_fixing_1180
        0x28b463ead0f4aaf0, // tab_fixed_code_1180
        0xe9e0e1db4f7379c5, // tab_fixed_code_860
        0x39dad694534611c7, // tab_approved_1180
        0x1f0a0d2a92104906, // tab_locked_1180
        0xfc861ee78304bfbc, // tab_start_over_1180
        0xad2f66c417c0d574, // tab_discover_brief_1180
        0x8210c973d4a3e76d, // tab_discover_brief_860
        0xe478e5d0bfc203ff, // tab_plan_long_1180
        0xa0b64a8d9cb72827, // tab_manual_test_1180
        0xa873580051f4643e, // tab_manual_note_860
        0x3f93fa45c78f5866, // tab_review_rounds_1180
        0x0f3df9cc94cd9bd0, // tab_verify_fixes_1180
        0x9a3eb2d9b749f166, // tab_verify_fixes_860
    ],
    // Dark at 2
    [
        0x0d7bb23945e58d57, // tab_build_860
        0x545381491e68b809, // tab_build_1180
        0xf5ca19121037fb0a, // tab_build_1920
        0xd87da022045a2089, // tab_plan_ready_1180
        0xf060b4300361f434, // tab_verify_failed_1920
        0x52a5d1ad9dffb717, // tab_verify_running_1180
        0x66b1c34acb71140e, // tab_fix_plan_860
        0x3682781e663407d6, // tab_ship_blocked_1180
        0x6d13e764c0086335, // tab_ship_routes_1180
        0xe158f1612e5697d6, // tab_ship_routes_860
        0x788a810c80dfa9ce, // tab_ship_finish_1180
        0xa06ece6f31277e6c, // tab_done_1920
        0x7f7a0f831318ce48, // tab_later_860
        0x9a7ddcf7422a85a9, // tab_repos_1180
        0xed2c7e1819bcfed2, // tab_unfolded_860
        0x3f5fd4025379d423, // tab_unfolded_1920
        0x9b43722f12b2c738, // tab_shipped_1180
        0x24e84007a1b67d49, // tab_repos_failed_1180
        0x8a7c34c255b21042, // tab_repos_error_860
        0x38380dc1021b4b95, // tab_more_repos_850
        0x2c4437c54bf7a9ae, // tab_more_repos_mid_850
        0x6c18c1e5723992b5, // tab_rail_wrap_500
        0x94abd38c6a527526, // tab_rail_wrap_1180
        0x7e207be37044cbc5, // tab_picks_later_1180
        0xdf8abca15895bc9b, // tab_picks_running_860
        0xffcd124a220e3b5e, // tab_model_later_1180
        0x3c9aad3f3ac64f60, // tab_model_long_860
        0x768057f7dca79eba, // tab_t3_draft_860
        0x936a9a9d38981d93, // tab_milestone_mode_1180
        0x97f3dccc4490c3fa, // tab_milestone_mode_860
        0xd6daee894876f1e5, // tab_held_860
        0xa614cc43a9952056, // tab_resume_1180
        0xf054db57d3898f64, // tab_resume_860
        0x084830ff463a6994, // tab_fixing_1180
        0x7cd23b8e3b040d91, // tab_fixed_code_1180
        0xb9061501fe219a87, // tab_fixed_code_860
        0xbfd1ff58f63dabc7, // tab_approved_1180
        0x2ac9eea070a89ec7, // tab_locked_1180
        0x14684a4d8940be84, // tab_start_over_1180
        0x5f23c0dfbcaab18c, // tab_discover_brief_1180
        0xacea8b7421947944, // tab_discover_brief_860
        0x9bf3d7ef05b66203, // tab_plan_long_1180
        0x0381a16f4c182e79, // tab_manual_test_1180
        0x69602a3c18bd7031, // tab_manual_note_860
        0x8f279db4c0377c33, // tab_review_rounds_1180
        0x5f77f0932430066e, // tab_verify_fixes_1180
        0xbbbee10e3a49cc30, // tab_verify_fixes_860
    ],
    // Light at 1
    [
        0x0ce645a86801977b, // tab_build_860
        0xd7e7901f7634429d, // tab_build_1180
        0x1665a96aaee281ee, // tab_build_1920
        0x1012d3e4c8093338, // tab_plan_ready_1180
        0x1446efceff05c114, // tab_verify_failed_1920
        0xb9f4489df4669939, // tab_verify_running_1180
        0x0ed0f57c474cc6c7, // tab_fix_plan_860
        0xd59032fb24dd423d, // tab_ship_blocked_1180
        0x646e11565c9de4e0, // tab_ship_routes_1180
        0xcfa8d9cdcb053e97, // tab_ship_routes_860
        0xed0d4cc441828359, // tab_ship_finish_1180
        0x667c5c9a3d2413e7, // tab_done_1920
        0x7363a198125024f1, // tab_later_860
        0x1767b919b7c8a0ea, // tab_repos_1180
        0x3fe8ea7dfd6a54b7, // tab_unfolded_860
        0xc3eed2e1e5f692a4, // tab_unfolded_1920
        0xfe3e5d891d83a895, // tab_shipped_1180
        0x3161f4810db9b78f, // tab_repos_failed_1180
        0x25b971c2204f6cae, // tab_repos_error_860
        0xe00302d7f223b863, // tab_more_repos_850
        0x2662887127978191, // tab_more_repos_mid_850
        0x8fc539bd294a1ab2, // tab_rail_wrap_500
        0xc5a8808822bb934a, // tab_rail_wrap_1180
        0xe4c3a23900240a34, // tab_picks_later_1180
        0xbd1f008362e31963, // tab_picks_running_860
        0x845dad8ceed3d6a2, // tab_model_later_1180
        0xfeeb9ec8dc02319b, // tab_model_long_860
        0x51bfb58b32dc5fe0, // tab_t3_draft_860
        0x2d3345b2a5173b95, // tab_milestone_mode_1180
        0xd385df5f368da9f9, // tab_milestone_mode_860
        0x5d758a3375ad59b4, // tab_held_860
        0xa79bf4b0e5c7222b, // tab_resume_1180
        0x823ad24610c8d561, // tab_resume_860
        0xc07b53ee03db295e, // tab_fixing_1180
        0xba2e02d2f9638e9c, // tab_fixed_code_1180
        0x6957a114160bdc75, // tab_fixed_code_860
        0xca21abeb42b40c90, // tab_approved_1180
        0xf8462e772a8bfb79, // tab_locked_1180
        0x6101ae175d95f95d, // tab_start_over_1180
        0x361a521b58676049, // tab_discover_brief_1180
        0xde0d608a5eccfeec, // tab_discover_brief_860
        0x77767b97a4381cff, // tab_plan_long_1180
        0xe4bd695e759f1b44, // tab_manual_test_1180
        0x061ec3807f66b135, // tab_manual_note_860
        0xe29b7adc8372deb1, // tab_review_rounds_1180
        0x06497f7e81355840, // tab_verify_fixes_1180
        0xfaef144a9cf2f302, // tab_verify_fixes_860
    ],
    // Light at 2
    [
        0xa00f642ec3c54e2f, // tab_build_860
        0x7f40d44968ffbe15, // tab_build_1180
        0x2a16d342affb412e, // tab_build_1920
        0x9ba97245b9431b1d, // tab_plan_ready_1180
        0xc0a2a63f8deefc51, // tab_verify_failed_1920
        0x1d13b9e1270b0ffc, // tab_verify_running_1180
        0x28cf2e2968f136f7, // tab_fix_plan_860
        0xd30b4e34c27b0745, // tab_ship_blocked_1180
        0xfd28fbee1d3acdfc, // tab_ship_routes_1180
        0xeb73d57026c5fc93, // tab_ship_routes_860
        0x4f0e14cfecc7b814, // tab_ship_finish_1180
        0xcf3c3a2235a70b9c, // tab_done_1920
        0x866b8ccb16687686, // tab_later_860
        0x5826cea00b665aaf, // tab_repos_1180
        0xac8efedf79d35257, // tab_unfolded_860
        0xa89e64e3836e4a55, // tab_unfolded_1920
        0xb2ee50aa0a138d22, // tab_shipped_1180
        0x8c18a07558083854, // tab_repos_failed_1180
        0x17f99877792cd662, // tab_repos_error_860
        0x3a22e4692f3d3e3c, // tab_more_repos_850
        0x9c947f89a572aa4e, // tab_more_repos_mid_850
        0x7ce202c070e50fb1, // tab_rail_wrap_500
        0xec15151a8bad58a5, // tab_rail_wrap_1180
        0x3500dae802999418, // tab_picks_later_1180
        0x1815a6fe842e0653, // tab_picks_running_860
        0x86e456cd43e28586, // tab_model_later_1180
        0xd5dd7c124a2ef7ac, // tab_model_long_860
        0x0e9955036c1a7725, // tab_t3_draft_860
        0xb7a18eff1e35cf77, // tab_milestone_mode_1180
        0xc135bc1204b837d7, // tab_milestone_mode_860
        0x9cce9a5aa8d18670, // tab_held_860
        0xc5c23bc0a12c240e, // tab_resume_1180
        0xba0f501e1d90ab88, // tab_resume_860
        0x24da56938b2370be, // tab_fixing_1180
        0x718dd7b5dba2ffd4, // tab_fixed_code_1180
        0x06ba5880b8ac5b69, // tab_fixed_code_860
        0xaf6e62e9aa28626c, // tab_approved_1180
        0xe88210491ec9da6d, // tab_locked_1180
        0xe8e6a8189d0001cd, // tab_start_over_1180
        0x1f0f7201920e14f6, // tab_discover_brief_1180
        0x6772f0fb23152ac2, // tab_discover_brief_860
        0x4c5a60823adfe9ae, // tab_plan_long_1180
        0x1e2c584646dc9ba6, // tab_manual_test_1180
        0x1b69d7c51f346529, // tab_manual_note_860
        0xc202861bd8801f45, // tab_review_rounds_1180
        0xf22eb3de2d0ec197, // tab_verify_fixes_1180
        0x6f6f159577e44677, // tab_verify_fixes_860
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
    for (theme, scale) in CASES {
        for (name, view) in feature_tabs() {
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

// ---------------------------------------------------- AI review's findings

/// The feature tab on AI review with a card of each kind, in the page's order:
/// the user's own, a finding to decide, one dismissed and closed, and one to
/// fix with the keyboard.
fn tab_findings() -> super::FeatureTabView {
    use super::{ButtonStyle, FindingCard, FindingTag, PageButton, PageSection};
    let mut view = feature_tab();
    view.focus = super::TabArea::Findings;
    view.page.title = "AI review".into();
    view.page.about = "Another model reviews the diff".into();
    view.page.buttons.clear();
    view.page.sections.clear();
    #[allow(clippy::type_complexity)]
    let cards: [(&[&str], FindingTag, &str, &str, &str, &str, &str, &str); 4] = [
        (
            &["Edit\u{2026}", "Delete"],
            FindingTag::Yours,
            "Rename retry",
            "to fix",
            "u1",
            "It reads badly.",
            "",
            "",
        ),
        (
            &["Fix", "Edit fix\u{2026}", "Dismiss\u{2026}", "Show in diff"],
            FindingTag::High,
            "Retry stops one short",
            "",
            "r1-1 \u{b7} login.py:6",
            "The loop runs range(n - 1), so the last try never happens, and a caller asking for \
             one retry gets none at all.",
            "Fix: Stop at n.",
            "",
        ),
        (
            &["Reopen", "Comment", "Show in diff"],
            FindingTag::High,
            "Unused import left behind by the last refactor of the login page",
            "dismissed: gone in the next branch \u{b7} commented",
            "r1-3 \u{b7} login.py:1",
            "",
            "",
            "",
        ),
        (
            &[
                "Don't fix",
                "Edit fix\u{2026}",
                "Dismiss\u{2026}",
                "Show in diff",
            ],
            FindingTag::Medium,
            "Title breaks with no name",
            "to fix",
            "r1-2 \u{b7} login.py:2",
            "A user with no name raises AttributeError.",
            "Your fix: Fall back to the plain title",
            "Mind the tests, and the greeting when the name is only spaces: it should fall back too.",
        ),
    ];
    for (i, (labels, tag, title, state, place, body, fix, note)) in cards.into_iter().enumerate() {
        let at = view.page.buttons.len();
        view.page
            .buttons
            .extend(labels.iter().map(|label| PageButton {
                label: (*label).to_owned(),
                style: ButtonStyle::Plain,
            }));
        let end = view.page.buttons.len();
        // The comment's own buttons, on it.
        let comment_buttons: Vec<usize> = if note.is_empty() {
            Vec::new()
        } else {
            view.page
                .buttons
                .extend(["Edit", "Delete"].map(|label| PageButton {
                    label: label.to_owned(),
                    style: ButtonStyle::Plain,
                }));
            (end..end + 2).collect()
        };
        view.page.sections.push(PageSection::Finding(FindingCard {
            tag,
            title: title.into(),
            state: state.into(),
            place: place.into(),
            body: body.into(),
            preview: Vec::new(),
            fix: fix.into(),
            note: String::new(),
            comment: note.into(),
            comment_buttons,
            made: false,
            faded: state.starts_with("dismissed"),
            collapsed: state.starts_with("dismissed"),
            stripe: (state == "to fix").then_some(super::CardStripe::ToFix),
            field: None,
            field_hint: "",
            selected: i == 3,
            buttons: (at..at + labels.len()).collect(),
        }));
    }
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
            made: false,
            faded: tag == FindingTag::Skipped,
            collapsed: false,
            stripe: None,
            field: None,
            field_hint: "",
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
            },
            placeholder,
            rows,
        }
    };
    super::FindingDialogView {
        title: "Edit the fix of r1-2".into(),
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
        error: error.then(|| "Say what to fix.".to_owned()),
        hover: None,
        keys: vec![
            ("Tab", "next"),
            ("Shift+Enter", "new line"),
            ("Esc", "close"),
        ],
    }
}

const FINDINGS: [&str; 4] = [
    "tab_findings_1180",
    "tab_findings_860",
    "finding_dialog",
    "finding_dialog_error",
];

/// The recorded hashes of `FINDINGS`, per case in `CASES`.
const GOLDEN_FINDINGS: [[u64; 4]; 4] = [
    // Dark at 1
    [
        0x92c31bd180b86dd5, // tab_findings_1180
        0x3cfbab9775e7b2b9, // tab_findings_860
        0xc8e97861327a6f3d, // finding_dialog
        0x79e255ab5a829143, // finding_dialog_error
    ],
    // Dark at 2
    [
        0x74eaa7e08adb40e5, // tab_findings_1180
        0xb48d6a0e3e3b67ae, // tab_findings_860
        0x43f1532742f932ff, // finding_dialog
        0xb736e6151427fb01, // finding_dialog_error
    ],
    // Light at 1
    [
        0x7d89cce6b7c71ce5, // tab_findings_1180
        0x41c9d7df8802233d, // tab_findings_860
        0x3af0633810888402, // finding_dialog
        0x2f19c19c900552df, // finding_dialog_error
    ],
    // Light at 2
    [
        0xd093678f17c9523f, // tab_findings_1180
        0x1670c3df91937aee, // tab_findings_860
        0xa9c6bc076a8eb05a, // finding_dialog
        0x1e721b2638489f69, // finding_dialog_error
    ],
];

#[test]
fn ai_reviews_findings_paint_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_FINDINGS.iter().map(|row| row.as_slice()).collect();
    let view = tab_findings();
    check_each(&FINDINGS, &golden, |painter, theme, scale, surface| {
        let tab = |w: i32| {
            super::feature_tab_composed(painter, &view, (w, TAB_HEIGHT).into(), theme, scale)
        };
        let dialog = |error: bool| {
            let size = IDENTITY_OUTPUT.into();
            painter.finding_dialog(&finding_dialog(error), size, scale, theme.palette())
        };
        hash(match surface {
            0 => tab(1180),
            1 => tab(860),
            2 => dialog(false),
            _ => dialog(true),
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
    let view = tab_findings();
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
    let detail = match on {
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
    };
    let head = |severity: &str, state: Option<&str>, tone, place: &str| DetailRow::CommentHead {
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
            finding: None,
            place: "comment 1 of 1".into(),
            within: None,
        },
        line("Why status here? Check the caller.", true, None),
        head("high", None, Tone::Error, "finding 1 of 4"),
        line("The parser splits on spaces", false, Some(Tone::Error)),
        line("Fix: split on NUL", true, Some(Tone::Error)),
        head("medium", Some("to fix"), Tone::Warning, "finding 2 of 4"),
        line("No timeout on the status read", false, Some(Tone::Warning)),
        DetailRow::CommentHead {
            finding: None,
            place: String::new(),
            within: Some(Tone::Warning),
        },
        DetailRow::Comment {
            text: "Keep it under a second".into(),
            first: false,
            last: true,
            tone: None,
            within: Some(Tone::Warning),
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
                button(ReposButton::MakeFixes, waits),
            ],
            vec![
                keyed(ReposButton::AddComment, "Shift+C", None),
                keyed(ReposButton::FixAllFindings, "Shift+F", None),
                keyed(ReposButton::MakeFixes, "Ctrl+Enter", waits),
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

fn doc_tabs() -> [(&'static str, super::ReposView, i32); 14] {
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
    ]
}

const DOC_TAB: [&str; 14] = [
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
const GOLDEN_DOC_TAB: [[u64; 14]; 4] = [
    // Dark at 1
    [
        0xe3291f02faaf07c4, // doc_preview_1180
        0x4af9130a9b0c1e78, // doc_review_1180
        0xd56be0a254359a36, // doc_review_no_browser
        0x163df7c54996fb12, // doc_preview_1920
        0x146ef2c66d529cd6, // doc_split_1920
        0x8dc4abbd4a4453aa, // doc_review_850
        0x991e984d1cf58986, // doc_approved_1180
        0xc795c2ffa685e118, // doc_locked_1180
        0x3f15d5f92f9eda75, // findings_1180
        0x8fadfc4bbd01f8f3, // popup_deciding_1180
        0xbe2c435b869d8d35, // popup_decided_1180
        0x6f1041c7f50aca45, // popup_code_review_850
        0x07ad73f54e3e101c, // popup_no_comments_1180
        0x08ab19690c613d25, // popup_no_comments_850
    ],
    // Dark at 2
    [
        0xb80f9bdca4ec1495, // doc_preview_1180
        0xd15dbc2eeae8d5cc, // doc_review_1180
        0x7c58f8622b115083, // doc_review_no_browser
        0xe1f51d6886965409, // doc_preview_1920
        0xa843db297b42f35c, // doc_split_1920
        0x0d72543f5d6587c5, // doc_review_850
        0xc29bb6d56b5beb67, // doc_approved_1180
        0x8a385f9aa43a43b8, // doc_locked_1180
        0x700557f5f8cc9865, // findings_1180
        0x78c4b839e155a578, // popup_deciding_1180
        0x7eb4dfd72d1f662a, // popup_decided_1180
        0x2ebeec0af5c2ef1b, // popup_code_review_850
        0x04f0f9c1258af57a, // popup_no_comments_1180
        0x654aa94035b7bbae, // popup_no_comments_850
    ],
    // Light at 1
    [
        0xa18686a06ead007c, // doc_preview_1180
        0xb82c5e0088f5f4c2, // doc_review_1180
        0x64bb2e6df8dd639f, // doc_review_no_browser
        0xf113a8e16264d641, // doc_preview_1920
        0x6e494dd4063c4bf6, // doc_split_1920
        0x59eff04f58c92d40, // doc_review_850
        0x4d6fa62f8b977f1d, // doc_approved_1180
        0xf1603167ed70f699, // doc_locked_1180
        0xacaed3527aa1f4f0, // findings_1180
        0xc318b335f76aa914, // popup_deciding_1180
        0x53fda247fb4f2a78, // popup_decided_1180
        0xdc7251d500deedf7, // popup_code_review_850
        0x078b0def98e11797, // popup_no_comments_1180
        0x0f750109c62c42b8, // popup_no_comments_850
    ],
    // Light at 2
    [
        0x2ac5806777ef660a, // doc_preview_1180
        0x6e1716682c599e65, // doc_review_1180
        0xa3e94f33bb76c764, // doc_review_no_browser
        0x04633b82213dc5ff, // doc_preview_1920
        0x8d1275261b45972b, // doc_split_1920
        0x6d05f471266e78e8, // doc_review_850
        0x84732f985b636d39, // doc_approved_1180
        0x153567080b2689aa, // doc_locked_1180
        0x5688ac50adad2001, // findings_1180
        0x12cb6fc6385c6fea, // popup_deciding_1180
        0xccffdce2a40e4dea, // popup_decided_1180
        0x648afae740203419, // popup_code_review_850
        0x2435e6adfbb4e6b8, // popup_no_comments_1180
        0x958a852299d3f6a4, // popup_no_comments_850
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

const CHECK_CARDS: [&str; 2] = ["tab_checks_1180", "tab_checks_860"];

/// The recorded hashes of `CHECK_CARDS`, per case in `CASES`.
const GOLDEN_CHECK_CARDS: [[u64; 2]; 4] = [
    [0x21b453e2d0c2ba9a, 0xa6741dfbd9e5f310],
    [0x7e4ef9f9c9837727, 0x669bca54ffe6a240],
    [0x45487a5a370e2db7, 0xc42764dd56db19ab],
    [0xbef32e1c1f41926a, 0x9a8985c824ad4a1d],
];

#[test]
fn verifys_check_cards_paint_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_CHECK_CARDS
        .iter()
        .map(|row| row.as_slice())
        .collect();
    let view = tab_checks();
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
    let view = tab_checks();
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
