//! Pixel-identity checks: every chrome surface painted from a fixed view,
//! at scale 1 and 2 in both themes, hashed. The hashes change only when
//! the drawing does; a change made on purpose records the new ones (run
//! the test and copy the table it prints).

use std::time::Instant;

use super::{
    AgentLine, AgentView, BarButton, BarButtons, BarStatus, FormField, FormView, Help, HelpHit,
    HelpPress, InboxButton, InboxDetailView, InboxIcon, InboxLine, InboxRow, InboxTone, InboxView,
    ItemIcon, MenuEntry, MenuHit, MenuRow, MenuView, Painter, PaneBarView, PlaceholderButton,
    PlaceholderView, ProjectMenuView, ProjectRow, QuitHit, QuitRow, QuitView, Rect, RowState,
    SidebarState, SidebarView, SwitcherRow, SwitcherView, TabStripView, TabView, Theme, ThemeMode,
    ToastView, TooltipView, WelcomeHit, WelcomeView, WorkspaceView,
};
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
            stages: Vec::new(),
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
            stages: Vec::new(),
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
const OVERLAYS: [&str; 10] = [
    "full_menu",
    "help_pinned",
    "toast_action",
    "tooltip",
    "placeholder",
    "placeholder_timed_out",
    "spinner",
    "blink_dot",
    "pane_pill",
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
        0x0862dbf6fad20a20, // sidebar
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
        0x8bbf6c3a7d553224, // sidebar
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
        0x0689b1e56874ec15, // sidebar
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
        0x15171197a99d5a9d, // sidebar
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
const GOLDEN_OVERLAYS: [[u64; 10]; 4] = [
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
        0x3973c1a54dd3decd, // pane_handle
    ],
];

/// Check `paint` of every case against `golden`, printing the table to
/// record when any surface paints differently.
fn check(surfaces: &[&str], golden: &[&[u64]], paint: impl Fn(&Painter, Theme, f64) -> Vec<u64>) {
    let painter = Painter::new();
    assert!(painter.font.is_some(), "the golden hashes need a font");
    let mut table = String::new();
    let mut same = true;
    for ((theme, scale), want) in CASES.into_iter().zip(golden) {
        let got = paint(&painter, theme, scale);
        table += &format!("    // {theme:?} at {scale}\n    [\n");
        for (i, (surface, hash)) in surfaces.iter().zip(&got).enumerate() {
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
            if touched { "changed" } else { "context" }
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
        link: Some(super::LINK_BUTTON[0]),
        repos: vec![
            repo("api", "dev", true),
            repo("shared", "main", false),
            repo("web", "dev", true),
            repo("billing: ledger", "main", false),
        ],
        go: "Create",
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

/// The feature panel on a feature over `repos` repositories (one alone
/// is the single-repository case), two stages done, Build current.
fn feature_panel(repos: usize) -> super::FeaturePanelView {
    use super::{FeatureRepoRow, StageMark, StageRow};
    use crate::feature::Stage;
    let names = ["api", "shared", "web"];
    let repos = (0..repos)
        .map(|i| FeatureRepoRow {
            name: names[i % names.len()].into(),
            line: if i == 1 {
                "context at dev".into()
            } else {
                "feature/bulk-export".into()
            },
            touched: i != 1,
            busy: false,
            button: (i == 1).then_some("Touch"),
        })
        .collect();
    let stages = Stage::ALL
        .iter()
        .enumerate()
        .map(|(i, stage)| {
            let (mark, detail) = match i {
                0 | 1 => (StageMark::Done, "done 2h ago by you".to_owned()),
                2 => (StageMark::Current, stage.about().to_owned()),
                _ => (StageMark::Later, stage.about().to_owned()),
            };
            StageRow {
                label: stage.label().into(),
                detail,
                button: match mark {
                    StageMark::Done => Some("Reopen"),
                    StageMark::Current => Some("Mark done"),
                    StageMark::Later => None,
                },
                mark,
            }
        })
        .collect();
    let hints = [
        ("Enter", "mark done"),
        ("Ctrl+O", "open tab"),
        ("Ctrl+C", "copy path"),
        ("Ctrl+N", "new"),
        ("Del", "abandon"),
    ];
    super::FeaturePanelView {
        title: "Bulk export of invoices".into(),
        sub: "bulk-export \u{b7} ~/Work/aft/.features/bulk-export".into(),
        which: Some((0, 2)),
        repos,
        stages,
        cursor: 0,
        repo_top: 0,
        remove: "Abandon\u{2026}",
        hover: None,
        hints: hints.map(|(k, a)| (k.to_owned(), a.to_owned())).into(),
        check: Vec::new(),
    }
}

/// The feature panel on three repositories with Ship current, the cursor
/// on it, showing the Ship check `check`; `web`'s push isn't guarded.
fn ship_panel(check: Vec<super::CheckLine>) -> super::FeaturePanelView {
    use super::{StageMark, StageRow};
    use crate::feature::Stage;
    let mut view = feature_panel(3);
    view.repos[2].line = "feature/bulk-export \u{b7} push not guarded: hooks path is set".into();
    view.stages = Stage::ALL
        .iter()
        .map(|stage| {
            let ship = *stage == Stage::Ship;
            StageRow {
                label: stage.label().into(),
                detail: if ship {
                    stage.about().to_owned()
                } else {
                    "done 2h ago by you".to_owned()
                },
                mark: if ship {
                    StageMark::Current
                } else {
                    StageMark::Done
                },
                button: Some(if ship { "Ship\u{2026}" } else { "Reopen" }),
            }
        })
        .collect();
    view.cursor = 3 + Stage::ALL.len() - 1;
    view.hints[0] = ("Enter".into(), "ship\u{2026}".into());
    view.check = check;
    view
}

/// Ship current, two things stopping it.
fn feature_ship_blocked() -> super::FeaturePanelView {
    use super::{CheckLine, CheckTone};
    ship_panel(vec![
        CheckLine {
            text: "api has changes not committed".into(),
            tone: CheckTone::Blocker,
        },
        CheckLine {
            text: "web is 3 commits behind dev".into(),
            tone: CheckTone::Blocker,
        },
    ])
}

/// Ship current and ready, with a note.
fn feature_ship_ready() -> super::FeaturePanelView {
    use super::{CheckLine, CheckTone};
    ship_panel(vec![
        CheckLine {
            text: "Ready to ship".into(),
            tone: CheckTone::Ready,
        },
        CheckLine {
            text: "web opens a pull request into dev".into(),
            tone: CheckTone::Note,
        },
    ])
}

/// A done feature whose pull requests are open, the cursor on one.
fn feature_done_prs() -> super::FeaturePanelView {
    use super::{StageMark, StageRow};
    use crate::feature::Stage;
    let mut view = feature_panel(3);
    for (i, repo) in ["api", "web"].into_iter().enumerate() {
        let row = &mut view.repos[i * 2];
        row.line = format!("feature/bulk-export \u{b7} PR {repo}/pull/1{i}");
        row.button = Some("Open PR");
    }
    view.stages = Stage::ALL
        .iter()
        .map(|stage| StageRow {
            label: stage.label().into(),
            detail: "done 2h ago by you".into(),
            mark: StageMark::Done,
            button: Some("Reopen"),
        })
        .collect();
    view.sub = format!("done \u{b7} {}", view.sub);
    view.remove = "Remove worktrees";
    let hints = [
        ("Up / Down", "move"),
        ("Enter", "open tab"),
        ("Ctrl+Shift+O", "open PR"),
        ("Ctrl+O", "open tab"),
        ("Ctrl+C", "copy path"),
        ("Ctrl+N", "new"),
        ("Del", "remove worktrees"),
        ("Esc", "close"),
    ];
    view.hints = hints.map(|(k, a)| (k.to_owned(), a.to_owned())).into();
    view
}

/// Eight stage marks, the first `done` of them done.
fn stage_marks(done: usize) -> Vec<super::StageMark> {
    use super::StageMark;
    (0..8)
        .map(|i| match i {
            i if i < done => StageMark::Done,
            i if i == done => StageMark::Current,
            _ => StageMark::Later,
        })
        .collect()
}

/// The sidebar with a feature's own workspace, its header showing the
/// feature's title and stage marks (the pointer on them), and another
/// feature with none open under the workspaces.
fn feature_workspace_sidebar() -> SidebarView {
    use super::FeatureRowView;
    let mut workspaces = workspaces();
    let feature = &mut workspaces[1];
    feature.name = "Bulk export of invoices".into();
    feature.stages = stage_marks(3);
    SidebarView {
        workspaces,
        agents: agents(),
        active: Some(1),
        project: Some("aft".into()),
        header_lit: false,
        features: vec![FeatureRowView {
            title: "Faster search".into(),
            marks: stage_marks(1),
        }],
        state: SidebarState {
            rows: vec![
                RowState::default(),
                RowState {
                    hover: 16,
                    stages: true,
                    ..RowState::default()
                },
            ],
            ..SidebarState::default()
        },
    }
}

/// The sidebar with two open features under its workspaces, the second
/// under the pointer.
fn features_sidebar() -> SidebarView {
    use super::FeatureRowView;
    let marks = stage_marks;
    SidebarView {
        workspaces: workspaces(),
        agents: agents(),
        active: Some(0),
        project: Some("aft".into()),
        header_lit: false,
        features: vec![
            FeatureRowView {
                title: "Bulk export of invoices".into(),
                marks: marks(2),
            },
            FeatureRowView {
                title: "A feature with a title far too long to fit".into(),
                marks: marks(6),
            },
        ],
        state: SidebarState {
            features: vec![0, 16],
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

const FEATURE_OUTPUT: (i32, i32) = (870, 720);

fn paint_features(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    let size = FEATURE_OUTPUT.into();
    vec![
        hash(painter.new_feature_dialog(&new_feature(), size, scale, p)),
        hash(painter.feature_panel(&feature_panel(1), size, scale, p)),
        hash(painter.feature_panel(&feature_panel(3), size, scale, p)),
        hash(painter.sidebar_view(&features_sidebar(), (260, 520).into(), scale, p)),
        hash(painter.feature_panel(&feature_ship_blocked(), size, scale, p)),
        hash(painter.feature_panel(&feature_ship_ready(), size, scale, p)),
        hash(painter.feature_panel(&feature_done_prs(), size, scale, p)),
        hash(painter.manual_test_sheet(&manual_test(false), size, scale, p)),
        hash(painter.manual_test_sheet(&manual_test(true), size, scale, p)),
        hash(painter.sidebar_view(&feature_workspace_sidebar(), (260, 520).into(), scale, p)),
        hash(painter.sidebar_view(&needs_you_sidebar(), (260, 520).into(), scale, p)),
    ]
}

/// The manual test sheet: a criterion of each kind, one cut short, the
/// last refusal said; `note`: a failure's note being written.
fn manual_test(note: bool) -> super::ManualTestView {
    use super::{ManualMark, ManualRow};
    let row = |id: &str, text: &str, mark, note: &str, sent| ManualRow {
        id: id.into(),
        text: text.into(),
        mark,
        note: note.into(),
        sent,
    };
    let hints: &[(&str, &str)] = if note {
        &[("Enter", "save"), ("Esc", "save")]
    } else {
        &[
            ("Space", "pass"),
            ("f", "fail\u{2026}"),
            ("Del", "clear"),
            ("Ctrl+Enter", "mark passed"),
            ("Ctrl+Shift+Enter", "send failures"),
            ("F1", "keys"),
            ("Esc", "close"),
        ]
    };
    super::ManualTestView {
        title: "Manual test of \u{201c}Bulk export of invoices\u{201d}".into(),
        sub: "1 of 4 criteria passed \u{b7} 2 failed".into(),
        rows: vec![
            row(
                "AC-1",
                "Given two invoices, when I export both, then one CSV holds both",
                ManualMark::Passed,
                "",
                false,
            ),
            row(
                "AC-2",
                "Given no invoices, when I export, then the button is off",
                ManualMark::Failed,
                "The button stays on and exports an empty file",
                true,
            ),
            row(
                "AC-3",
                "Given a cancelled invoice, when I export, then it is left out of the file and the total",
                ManualMark::Failed,
                "It shows with a total of 0",
                false,
            ),
            row(
                "AC-4",
                "Given 500 invoices, when I export, then it is done in a second",
                ManualMark::Untested,
                "",
                false,
            ),
        ],
        cursor: if note { 3 } else { 2 },
        top: 0,
        editing: note.then(|| (3, Line::from("Takes 4 seconds"))),
        notice: (!note).then(|| "Not passed yet: AC-2 failed, AC-3 failed, AC-4 not tried".into()),
        hover: (!note).then_some(super::ManualHit::Fail(3)),
        primary: Some(super::MANUAL_SEND),
        keys: None,
        hints: hints
            .iter()
            .map(|(k, w)| ((*k).to_owned(), (*w).to_owned()))
            .collect(),
    }
}

const FEATURES: [&str; 11] = [
    "new_feature",
    "feature_panel",
    "feature_panel_multi",
    "sidebar_features",
    "feature_ship_blocked",
    "feature_ship_ready",
    "feature_done_prs",
    "manual_test",
    "manual_test_note",
    "sidebar_feature_workspace",
    "sidebar_needs_you",
];

/// The recorded hashes of `FEATURES`, per case in `CASES`.
const GOLDEN_FEATURES: [[u64; 11]; 4] = [
    // Dark at 1
    [
        0xaf6537e36661773b, // new_feature
        0x2035a515e642ccfc, // feature_panel
        0x5b44ce1855fbeb6c, // feature_panel_multi
        0x5e017fbffabde99b, // sidebar_features
        0x8131a71ac2be99e8, // feature_ship_blocked
        0x6bc31ef9b6d52606, // feature_ship_ready
        0x25725cfbc3b3ba91, // feature_done_prs
        0xab580425d0a50879, // manual_test
        0x8e4d20f7c6316567, // manual_test_note
        0x6a43783f5bb5fcd3, // sidebar_feature_workspace
        0x818d05fb2ec3d9a9, // sidebar_needs_you
    ],
    // Dark at 2
    [
        0x9a3eb907776fe145, // new_feature
        0x78044619f7e91bd3, // feature_panel
        0x4d406c7a7a0d55f0, // feature_panel_multi
        0x095e2dfbb584cb35, // sidebar_features
        0xdfb59c820d65ece1, // feature_ship_blocked
        0x74a7d1af895962db, // feature_ship_ready
        0xc43b3e6d75230966, // feature_done_prs
        0xc14927ebcde55bf1, // manual_test
        0xb520dd662440b737, // manual_test_note
        0x6d690a720cee56d2, // sidebar_feature_workspace
        0x445f579930bc139c, // sidebar_needs_you
    ],
    // Light at 1
    [
        0x0e77512e4815b092, // new_feature
        0x0e728426227e94e8, // feature_panel
        0x0c3681e01ce58bb2, // feature_panel_multi
        0xd22590056de00ae5, // sidebar_features
        0x5631c29edb78f675, // feature_ship_blocked
        0xd424f6513eea33c5, // feature_ship_ready
        0x05cc7d58ebc3d6c6, // feature_done_prs
        0x093b890ce5537eef, // manual_test
        0xa74e12b563efbdaf, // manual_test_note
        0xd2bbdc87ca87827a, // sidebar_feature_workspace
        0x0884214637ba9738, // sidebar_needs_you
    ],
    // Light at 2
    [
        0x9e8eaff9dfeac632, // new_feature
        0x1bfbbfcdf4c610ce, // feature_panel
        0xbe2d06e390e5c642, // feature_panel_multi
        0x5e1381a968fcc82e, // sidebar_features
        0x22b990f6fe45e2d3, // feature_ship_blocked
        0x0db169edcb2bf4ad, // feature_ship_ready
        0x0a045e32666be0f4, // feature_done_prs
        0xed40d3ad66f63314, // manual_test
        0x1239c6f64e145f2f, // manual_test_note
        0x0998b64583e38b40, // sidebar_feature_workspace
        0x94e9eab9390bd4db, // sidebar_needs_you
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
        let views = [
            ("feature_panel", feature_panel(1)),
            ("feature_panel_multi", feature_panel(3)),
            ("feature_ship_blocked", feature_ship_blocked()),
            ("feature_ship_ready", feature_ship_ready()),
            ("feature_done_prs", feature_done_prs()),
        ];
        for (name, view) in views {
            let panel =
                super::feature_panel_layout(&view, smithay::utils::Rectangle::from_size(size))
                    .panel;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.feature_panel(&view, size, scale, p),
                px(panel.size.w + shadow),
            );
        }
        for (name, note) in [("manual_test", false), ("manual_test_note", true)] {
            let view = manual_test(note);
            let panel = super::manual_test_layout(&view, size).panel;
            dump(
                &dir,
                &format!("{name}_{tag}"),
                painter.manual_test_sheet(&view, size, scale, p),
                px(panel.size.w + shadow),
            );
        }
        dump(
            &dir,
            &format!("sidebar_features_{tag}"),
            painter.sidebar_view(&features_sidebar(), (260, 520).into(), scale, p),
            px(260),
        );
        dump(
            &dir,
            &format!("sidebar_feature_workspace_{tag}"),
            painter.sidebar_view(&feature_workspace_sidebar(), (260, 520).into(), scale, p),
            px(260),
        );
        dump(
            &dir,
            &format!("sidebar_needs_you_{tag}"),
            painter.sidebar_view(&needs_you_sidebar(), (260, 520).into(), scale, p),
            px(260),
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

// ------------------------------------------------------------ feature tab

/// How high the feature tab is painted, and the widths it is painted at:
/// the chips row under 870, no activity column under 1180, and all three
/// columns.
const TAB_HEIGHT: i32 = 900;
const TAB_WIDTHS: [i32; 3] = [860, 1180, 1920];

fn tab_line(text: &str, tone: super::CheckTone) -> super::CheckLine {
    super::CheckLine {
        text: text.into(),
        tone,
    }
}

/// A feature's tab with Build current and running: claude at work with
/// the tail of its pane, three of five milestones done, three
/// repositories (shared not touched).
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
                2 => "running 14m".into(),
                _ => String::new(),
            },
            mark: match i {
                0 | 1 => NodeMark::Done,
                2 => NodeMark::Current,
                _ => NodeMark::Later,
            },
            selected: i == 2,
        })
        .collect();
    let repo =
        |name: &str, line: &str, touched: bool, button: Option<&'static str>| FeatureRepoRow {
            name: name.into(),
            line: line.into(),
            touched,
            busy: false,
            button,
        };
    let activity = [
        ("Build started by you", "14m ago", CheckTone::Note),
        ("plan.md approved", "1h ago", CheckTone::Ready),
        (
            "brief.md reviewed, with two comments sent to the agent",
            "2h ago",
            CheckTone::Ready,
        ),
        ("web joined the feature", "3h ago", CheckTone::Note),
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
        repos: vec![
            repo("api", "feature/bulk-export", true, None),
            repo("web", "feature/bulk-export", true, None),
            repo("shared", "context at dev", false, Some("Touch")),
        ],
        repos_error: None,
        repo: 0,
        focus: TabArea::Stages,
        page: StagePage {
            title: "Build".into(),
            about: Stage::Build.about().into(),
            status: Some(tab_line("claude is working in tab 2", CheckTone::Note)),
            buttons: vec![
                PageButton {
                    label: "Mark done".into(),
                    style: ButtonStyle::Primary,
                    key: Some("\u{23ce}"),
                },
                PageButton {
                    label: "Restart agent".into(),
                    style: ButtonStyle::Plain,
                    key: None,
                },
                PageButton {
                    label: "Show agent".into(),
                    style: ButtonStyle::Plain,
                    key: None,
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
                    ],
                    button: Some(2),
                },
                PageSection::Milestones {
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
fn feature_tab_plan_review() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, Dot, NodeMark, PageButton, PageSection, StagePage};
    use crate::{agent_icons::AgentIcon, feature::Stage};
    let mut view = feature_tab();
    at_stage(&mut view, 1, NodeMark::Waiting, "ready for review");
    let button = |label: &str, style, key| PageButton {
        label: label.into(),
        style,
        key,
    };
    view.page = StagePage {
        title: "Plan".into(),
        about: Stage::Plan.about().into(),
        status: Some(tab_line("plan.md is ready for review", CheckTone::Blocker)),
        buttons: vec![
            button("Review", ButtonStyle::Primary, Some("\u{23ce}")),
            button("Preview", ButtonStyle::Plain, None),
            button("Show agent", ButtonStyle::Plain, None),
        ],
        sections: vec![
            PageSection::Agent {
                icon: AgentIcon::Codex,
                name: "codex".into(),
                dot: Dot::Idle,
                status: "waiting for you \u{b7} tab 2".into(),
                tail: Vec::new(),
                button: Some(2),
            },
            PageSection::Lines {
                label: "plan.md".into(),
                lines: vec![
                    tab_line("Ready for review, sent back once", CheckTone::Blocker),
                    tab_line("5 milestones", CheckTone::Note),
                ],
            },
        ],
    };
    view
}

/// Verify failed: two checks fail, a fix sent.
fn feature_tab_verify_failed() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, NodeMark, PageButton, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 3, NodeMark::Current, "failed");
    view.page = StagePage {
        title: "Verify".into(),
        about: Stage::Verify.about().into(),
        status: Some(tab_line(
            "2 of 5 checks failed \u{b7} fixes sent 1 of 3",
            CheckTone::Blocker,
        )),
        buttons: vec![
            PageButton {
                label: "Run again".into(),
                style: ButtonStyle::Primary,
                key: Some("\u{23ce}"),
            },
            PageButton {
                label: "Evidence".into(),
                style: ButtonStyle::Plain,
                key: None,
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

/// Ship current, blocked by what isn't committed and a branch behind.
fn feature_tab_ship_blocked() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, NodeMark, PageButton, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    at_stage(&mut view, 7, NodeMark::Current, "blocked");
    view.repos[1].line = "feature/bulk-export \u{b7} push not guarded: hooks path is set".into();
    view.page = StagePage {
        title: "Ship".into(),
        about: Stage::Ship.about().into(),
        status: Some(tab_line("2 things stop shipping", CheckTone::Blocker)),
        buttons: vec![PageButton {
            label: "Ship\u{2026}".into(),
            style: ButtonStyle::Off,
            key: Some("\u{23ce}"),
        }],
        sections: vec![
            PageSection::Lines {
                label: "Ship check".into(),
                lines: vec![
                    tab_line("api has changes not committed", CheckTone::Blocker),
                    tab_line("web is 3 commits behind dev", CheckTone::Blocker),
                ],
            },
            PageSection::Lines {
                label: "How it ships".into(),
                lines: vec![
                    tab_line("api: squash into main", CheckTone::Note),
                    tab_line("web: pull request into dev", CheckTone::Note),
                ],
            },
        ],
    };
    view
}

/// A done feature, Ship's page shown read-only, with its pull requests.
fn feature_tab_done() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, NodeMark, PageButton, PageSection, StagePage};
    use crate::feature::Stage;
    let mut view = feature_tab();
    for node in &mut view.rail {
        node.mark = NodeMark::Done;
        node.line = "2h ago".into();
    }
    view.rail[3].mark = NodeMark::Stale;
    view.rail[3].line = "code changed".into();
    select_node(&mut view, 7);
    for (i, repo) in ["api", "web"].into_iter().enumerate() {
        let row = &mut view.repos[i];
        row.line = format!("PR {repo}/pull/1{i}");
        row.button = Some("Open PR");
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
            key: None,
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
    select_node(&mut view, 5);
    view.hover = Some(super::FeatureTabHit::Node(6));
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

/// The keyboard on the repositories, on shared, the pointer on its Touch.
fn feature_tab_repos() -> super::FeatureTabView {
    let mut view = feature_tab();
    view.focus = super::TabArea::Repos;
    view.repo = 2;
    view.hover = Some(super::FeatureTabHit::RepoButton(2));
    view.hints[1] = ("Tab".into(), "stages".into());
    view.hints[2] = ("Enter".into(), "touch".into());
    view
}

/// The repositories couldn't be read.
fn feature_tab_repos_error() -> super::FeatureTabView {
    let mut view = feature_tab();
    view.repos_error = Some("git status in api: fatal: not a git repository".into());
    view
}

/// The feature tab's fixtures, by name.
fn feature_tabs() -> [(&'static str, super::FeatureTabView); 8] {
    [
        ("tab_build", feature_tab()),
        ("tab_plan_review", feature_tab_plan_review()),
        ("tab_verify_failed", feature_tab_verify_failed()),
        ("tab_ship_blocked", feature_tab_ship_blocked()),
        ("tab_done", feature_tab_done()),
        ("tab_later", feature_tab_later()),
        ("tab_repos", feature_tab_repos()),
        ("tab_repos_error", feature_tab_repos_error()),
    ]
}

/// Each fixture at the width it shows best at, and Build at all three.
const TAB_SURFACES: [(&str, i32); 10] = [
    ("tab_build", 860),
    ("tab_build", 1180),
    ("tab_build", 1920),
    ("tab_plan_review", 1180),
    ("tab_verify_failed", 1920),
    ("tab_ship_blocked", 1180),
    ("tab_done", 1920),
    ("tab_later", 860),
    ("tab_repos", 1180),
    ("tab_repos_error", 860),
];

const FEATURE_TAB: [&str; 10] = [
    "tab_build_860",
    "tab_build_1180",
    "tab_build_1920",
    "tab_plan_review_1180",
    "tab_verify_failed_1920",
    "tab_ship_blocked_1180",
    "tab_done_1920",
    "tab_later_860",
    "tab_repos_1180",
    "tab_repos_error_860",
];

fn paint_feature_tab(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    let views = feature_tabs();
    TAB_SURFACES
        .iter()
        .map(|(name, w)| {
            let view = &views.iter().find(|(n, _)| n == name).expect("a fixture").1;
            hash(painter.feature_tab(view, (*w, TAB_HEIGHT).into(), scale, p))
        })
        .collect()
}

/// The recorded hashes of `FEATURE_TAB`, per case in `CASES`.
const GOLDEN_FEATURE_TAB: [[u64; 10]; 4] = [
    // Dark at 1
    [
        0x348b84218afa171a, // tab_build_860
        0x55d13eec94bf4555, // tab_build_1180
        0xf9baba634a0dfde4, // tab_build_1920
        0x08efb8696a3d1fc7, // tab_plan_review_1180
        0x5b0bbb8b952a14a2, // tab_verify_failed_1920
        0x61f0b5b524725188, // tab_ship_blocked_1180
        0x1031f8708f1219e4, // tab_done_1920
        0x55e1e5240591705d, // tab_later_860
        0xedb630fd10dffba9, // tab_repos_1180
        0xededcb57e73a5710, // tab_repos_error_860
    ],
    // Dark at 2
    [
        0xaef0211514424fd5, // tab_build_860
        0x3405e14d5eae123d, // tab_build_1180
        0x1943f9d2e6fab4e8, // tab_build_1920
        0x63f25849ffdb0937, // tab_plan_review_1180
        0x8eea453de3cc7eaf, // tab_verify_failed_1920
        0xd8f1da5a92186ecb, // tab_ship_blocked_1180
        0x9f9afb52c5c6dcb8, // tab_done_1920
        0x5274ce4207fdc076, // tab_later_860
        0x341b9b5c4113dc4c, // tab_repos_1180
        0x443de08e3b631cd2, // tab_repos_error_860
    ],
    // Light at 1
    [
        0x22acff39d98c9080, // tab_build_860
        0x3c6d9f56837b4328, // tab_build_1180
        0xf939cbc6c0f95e86, // tab_build_1920
        0xb345f8e69fcc9334, // tab_plan_review_1180
        0xa785259417eab6b3, // tab_verify_failed_1920
        0xd2c2237e24eb3d1c, // tab_ship_blocked_1180
        0x7eefc7689a2a0df5, // tab_done_1920
        0x662bf20365391924, // tab_later_860
        0xb4d2d72147495c26, // tab_repos_1180
        0x7600bc6b202928a3, // tab_repos_error_860
    ],
    // Light at 2
    [
        0x7c02447b36b5f121, // tab_build_860
        0x35b122e118bc0cdf, // tab_build_1180
        0x4ce4f47fd2f9f33d, // tab_build_1920
        0x2090a6260fdd0304, // tab_plan_review_1180
        0xed7db5c147b43789, // tab_verify_failed_1920
        0x4b1992f273815083, // tab_ship_blocked_1180
        0x88a2ab2968583255, // tab_done_1920
        0x01a28f82a1d783bf, // tab_later_860
        0xc3e73655b1793684, // tab_repos_1180
        0x673c1e75b50d422b, // tab_repos_error_860
    ],
];

#[test]
fn the_feature_tab_paints_the_same_pixels() {
    let golden: Vec<&[u64]> = GOLDEN_FEATURE_TAB
        .iter()
        .map(|row| row.as_slice())
        .collect();
    check(&FEATURE_TAB, &golden, paint_feature_tab);
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
        let p = theme.palette();
        for (name, view) in feature_tabs() {
            for w in TAB_WIDTHS {
                dump(
                    &dir,
                    &format!("{name}_{w}_{}_{}", theme.name(), scale),
                    painter.feature_tab(&view, (w, TAB_HEIGHT).into(), scale, p),
                    (w as f64 * scale).round() as usize,
                );
            }
        }
    }
}
