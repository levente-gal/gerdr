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

fn pane_bar() -> PaneBarView {
    PaneBarView {
        rect: Rect::new((0, 0).into(), (600, super::PANE_BAR_HEIGHT).into()),
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
        label(I::Failed, "Ignored errors", 1, 0),
        {
            let mut row = inbox_item(
                "i1",
                "ValueError: tool 2 is already in use",
                "backend · bash ./runlocal.sh · ×55",
                I::Failed,
                T::Failed,
                "3m",
            );
            if let InboxRow::Item(line) = &mut row {
                line.hidden = true;
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
                state: SidebarState::default(),
            },
            (200, 400).into(),
            scale,
            p,
        )),
        hash(painter.tab_strip_view(
            &TabStripView::plain(tabs(), 1, true, ThemeMode::System),
            theme,
            strip,
            scale,
        )),
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
                crate::agent_icons::AgentIcon::Cursor,
                crate::agent_icons::AgentIcon::OpenCode,
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
        0x742820c5c927d818, // sidebar
        0x6f51308dd7cdce6c, // tab_strip
        0x5fee9efd7b4d90d0, // pane_bar
        0x866d59a70ebe252d, // help
        0x378d0ffdb2449b77, // menu
        0xfc09c643c2d57ae1, // switcher
        0xe4d55e0a322e116f, // welcome
        0x1fac2c9284ad07fa, // quit
        0x92a49c766cb37d1c, // project_menu
        0xaf72881e8b741219, // project_form
        0xbd0d423ca3de12bd, // toast
        0x027c482de5118d1d, // agent_strip
    ],
    // Dark at 2
    [
        0x9ddccb37c80d489b, // sidebar
        0x16e4f5440b99562b, // tab_strip
        0x45306f6b0cd01731, // pane_bar
        0x56045b72862ed173, // help
        0xd724a365fa94d169, // menu
        0x1a39f33e0a29f83b, // switcher
        0xc6a2b9324f6b7b75, // welcome
        0x91306b9e268dc79d, // quit
        0x68b0b29c55156a50, // project_menu
        0x1d108a7b493c0cd1, // project_form
        0x8a9a152ce2a17651, // toast
        0x2d0c586e0eece85f, // agent_strip
    ],
    // Light at 1
    [
        0x4833c6a51e833328, // sidebar
        0x979ae0979da03949, // tab_strip
        0x16f9e0cb60272263, // pane_bar
        0x8c0302567b35f0c3, // help
        0xf82050c3b2848862, // menu
        0x422064c274f92b90, // switcher
        0x232afdce7d51101b, // welcome
        0xc9fc61e2fab1872e, // quit
        0xaf88a1744cdd63ab, // project_menu
        0xe2e62fecf8cb83c6, // project_form
        0x3023720e0af5627f, // toast
        0x818421f428039355, // agent_strip
    ],
    // Light at 2
    [
        0xe9feefcbe02c7ed6, // sidebar
        0x0f133010f45d3831, // tab_strip
        0x1d9ebaa9665f8db5, // pane_bar
        0x22f444d49649f855, // help
        0xc17d1e64d3c77f97, // menu
        0x77eb0acf1f4d4fd5, // switcher
        0xc5b0dff47ac46d40, // welcome
        0x0a88810aa7ad751e, // quit
        0xad9c7bc5aa554ff4, // project_menu
        0x527909807a17afb5, // project_form
        0xd1410a8fb372b973, // toast
        0x14656a2796a8443a, // agent_strip
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
            painter.tab_strip_view(
                &TabStripView::plain(tabs(), 1, true, ThemeMode::System),
                theme,
                (900, strip_h).into(),
                scale,
            ),
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
