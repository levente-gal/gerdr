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
        0xe389c22057f526b6, // sidebar
        0xed1972252f36590d, // tab_strip
        0x666fd35c1955aaa6, // pane_bar
        0x866d59a70ebe252d, // help
        0x378d0ffdb2449b77, // menu
        0x222c5ec526350aca, // switcher
        0x8cb9e7e1c80a7231, // welcome
        0x2ffe79ab7ef15cce, // quit
        0xf28e73b0f19af0ae, // project_menu
        0x40b86d9394e2b0f6, // project_form
        0xbd0d423ca3de12bd, // toast
        0x1b80b690e340528a, // agent_strip
    ],
    // Dark at 2
    [
        0x97aae0caed2b919e, // sidebar
        0x75280e3af24f953f, // tab_strip
        0x0dc4d7db9b5967cd, // pane_bar
        0x56045b72862ed173, // help
        0xd724a365fa94d169, // menu
        0x6cc81b30f90cc5e6, // switcher
        0x7228b99386b303e1, // welcome
        0x0f8b61f4910134a3, // quit
        0x50eec8407b3dab46, // project_menu
        0xb106808fcdacd38e, // project_form
        0x8a9a152ce2a17651, // toast
        0x95f9334ab0c42dc0, // agent_strip
    ],
    // Light at 1
    [
        0x7725bbc9577f3e11, // sidebar
        0xf5c707c6dab41293, // tab_strip
        0xa514c9cbf0c81198, // pane_bar
        0x8c0302567b35f0c3, // help
        0xf82050c3b2848862, // menu
        0x1c55b0ad8b473feb, // switcher
        0xa1d750b5ccc720d1, // welcome
        0x7a7ace7eb4d8a736, // quit
        0x4e1920c645fb3bd9, // project_menu
        0xba3c6be2c453a2bf, // project_form
        0x3023720e0af5627f, // toast
        0xa37db337f4fcec5f, // agent_strip
    ],
    // Light at 2
    [
        0xc84048d64b689513, // sidebar
        0x65c73e85a9732c79, // tab_strip
        0x887475a089d81641, // pane_bar
        0x22f444d49649f855, // help
        0xc17d1e64d3c77f97, // menu
        0x8d2a4b7ab4c3d3b6, // switcher
        0x6286d26d1401730a, // welcome
        0x7986c8c8af717529, // quit
        0x597b19b650a6f61c, // project_menu
        0xe2c375293cea9c87, // project_form
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
        0x4e0bb89f143ae0d2, // tasks_panel
    ],
    // Dark at 2
    [
        0x910ba2519163ffe3, // footer
        0x8fcd41618448e585, // tasks_panel
    ],
    // Light at 1
    [
        0x8a8087fd34aec460, // footer
        0x73394c4c1f2ef470, // tasks_panel
    ],
    // Light at 2
    [
        0x28f7c3f0d800d54e, // footer
        0xfc7ddca895c0b3ab, // tasks_panel
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
/// same branch's suggested, one repository with none.
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
        0xe26d4051ade3c3c3, // new_feature
        0x2aff067e65002fc3, // new_feature_bases
        0x35ce95cae4a606c7, // sidebar_features
        0xcec44541e82a9fc9, // sidebar_feature_workspace
        0xec1b534e416312d9, // sidebar_needs_you
        0x38dbadad2308760e, // sidebar_no_features
        0x9b1ad4602d5dfe05, // new_review
        0xcb449e75ac357c2f, // new_review_pulls
    ],
    // Dark at 2
    [
        0x42067dce4e972385, // new_feature
        0x244864fbc4aa11bd, // new_feature_bases
        0xac77be79af90543a, // sidebar_features
        0x8fa4fa3fc01a005e, // sidebar_feature_workspace
        0xf1b7525a91f048f0, // sidebar_needs_you
        0xe6a933a97909111c, // sidebar_no_features
        0xeed5e65264de763e, // new_review
        0xe89e33c5b92d9e87, // new_review_pulls
    ],
    // Light at 1
    [
        0x31e0e39b14a088b3, // new_feature
        0x531fdeb26471714a, // new_feature_bases
        0xbb1032aea480ea43, // sidebar_features
        0x5d757742533ae5fe, // sidebar_feature_workspace
        0x414d843532499a58, // sidebar_needs_you
        0x474724f31cd435c8, // sidebar_no_features
        0x0a97b8d759cc4d36, // new_review
        0x5acb59355ac85e97, // new_review_pulls
    ],
    // Light at 2
    [
        0xea82b6972eed93d0, // new_feature
        0x25afcba58c23a38c, // new_feature_bases
        0xf2418f0f3ae45142, // sidebar_features
        0x219fe458a0ba299a, // sidebar_feature_workspace
        0x4b9d19393791d2df, // sidebar_needs_you
        0xde76d59c6519cad2, // sidebar_no_features
        0xb14912585970169a, // new_review
        0x3df8aaffc0ac4a05, // new_review_pulls
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
        0xbba1df5757ed413a, // identity
        0xc87e46c74c54f9a3, // identity_error
    ],
    // Dark at 2
    [
        0x870075a0e427ce44, // identity
        0xd9d2d0de357d9d81, // identity_error
    ],
    // Light at 1
    [
        0xbe719200f205e134, // identity
        0x745354b45290cdd2, // identity_error
    ],
    // Light at 2
    [
        0x32db364b2f819350, // identity
        0xbdb67fc735f555dc, // identity_error
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
        0xfbf2969785aee1f9, // machine
        0xf3cd997dec8c40a1, // machine_connecting
        0x8dbd5f8d6d6adce1, // machine_failed
    ],
    // Dark at 2
    [
        0xe9f8f8c3ad56a8b1, // machine
        0x43a93ca52ada3ae5, // machine_connecting
        0xa23cd61dddc9dea9, // machine_failed
    ],
    // Light at 1
    [
        0xc93638b8641818b0, // machine
        0xf060613c4e2d8a04, // machine_connecting
        0x29b9f298afe57470, // machine_failed
    ],
    // Light at 2
    [
        0x6788a45ce6162f94, // machine
        0x230b3c41e2777298, // machine_connecting
        0x0aa1768d5fafc79c, // machine_failed
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
        0x2b6bf4edd5630f87, // machines_connected
        0x045beb82993735e7, // machines_failed
        0x05f7f25179a25268, // machines_add
        0x97d49b5b1499999e, // machines_armed
        0xac9bd4743bcac9c2, // machines_edit
        0x3499790fbe30d146, // machines_edit_trying
        0xc9511a320db795c0, // machines_edit_failed
        0x452a91e962ac541e, // machines_edit_addresses
        0xfb75c83153d8c88f, // machines_same_machine
        0xad4912224b431eb6, // machines_placement
    ],
    // Dark at 2
    [
        0x9268e79c1b845abf, // machines_connected
        0xdc2033797711699e, // machines_failed
        0x2229d3395be7af1d, // machines_add
        0xceb60959e106c87d, // machines_armed
        0x5feed0fd844a1e4f, // machines_edit
        0xe10ff285cce6b820, // machines_edit_trying
        0x4da219a62e940dec, // machines_edit_failed
        0x999fae821d628445, // machines_edit_addresses
        0xc4d75d81c92aa347, // machines_same_machine
        0xac1a666d313f24b9, // machines_placement
    ],
    // Light at 1
    [
        0x86f3306140451403, // machines_connected
        0xfb4811f37a1d6ae3, // machines_failed
        0xc3b26aee9b3362a4, // machines_add
        0xbe436e58f64f01d4, // machines_armed
        0x9af880b282fde20f, // machines_edit
        0xc858b9aab67506e6, // machines_edit_trying
        0x4bb86144a755e396, // machines_edit_failed
        0x7f9f8c5419602d96, // machines_edit_addresses
        0xd95b554e58f2f26a, // machines_same_machine
        0x3796fdf9ab7d7d84, // machines_placement
    ],
    // Light at 2
    [
        0x4a60891c71cd9770, // machines_connected
        0x54a666fe3efb3460, // machines_failed
        0x15c02eedcc9ae0b3, // machines_add
        0xf2519e2d6407e61e, // machines_armed
        0xc837485eb40f56d8, // machines_edit
        0xd008f36aed7f3fba, // machines_edit_trying
        0xf90d8a84739e4d45, // machines_edit_failed
        0xe5abdd3393f73c72, // machines_edit_addresses
        0xe5c14e77205b3b64, // machines_same_machine
        0x6d92eb3972900c4b, // machines_placement
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
        0x233ac5e4d1e8fbe4, // restart
        0x7d631d6a6fe58b1e, // restart_sandbox
    ],
    // Dark at 2
    [
        0xe10f6500cd51352d, // restart
        0xf806409ebe486a0e, // restart_sandbox
    ],
    // Light at 1
    [
        0x87aa9167c4c13fcd, // restart
        0x013766ace403d915, // restart_sandbox
    ],
    // Light at 2
    [
        0x49976c581bfaa9c3, // restart
        0xf4a0fa4b65e1c0fd, // restart_sandbox
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
        0x8948934910003701, // commands
        0xe36104db016e0ed6, // commands_setup
        0x6eaaa9f78af5fb15, // commands_stale
        0xf49ab1be12a62367, // commands_low
        0x2c3fd9fa4109884a, // commands_run
        0xa0e9bc346c0817da, // commands_feature
    ],
    // Dark at 2
    [
        0x71a3270da89e0e47, // commands
        0xd9bb577893bcd7cd, // commands_setup
        0x5ea91e40f6792835, // commands_stale
        0xbdf723e3a366fbb0, // commands_low
        0xf13751978d084386, // commands_run
        0x97fe7637fbec54a9, // commands_feature
    ],
    // Light at 1
    [
        0xcd689f42fe3f3c88, // commands
        0x2a5b2af524fb6f5c, // commands_setup
        0x27856c7290db9bf1, // commands_stale
        0x85b0ff253543353d, // commands_low
        0x357b272456513d94, // commands_run
        0x597332502b26ab0b, // commands_feature
    ],
    // Light at 2
    [
        0xdc6500d5bdaead9c, // commands
        0x416312f9a5ac3fc4, // commands_setup
        0xe322415985254a50, // commands_stale
        0xd777ebd60fc712aa, // commands_low
        0x5877ebb209449bd6, // commands_run
        0x0c21e4134d23cd90, // commands_feature
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
                Some(("Squash; you edit the message in a tab", false)),
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
        ("Merge main in\u{2026}", ButtonStyle::Plain),
        ("Rebase\u{2026}", ButtonStyle::Quiet),
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
/// ways to take that on.
fn feature_tab_ship_conflicts() -> super::FeatureTabView {
    use super::{ButtonStyle, CheckTone, PageSection};
    let mut view = feature_tab_ship_cards();
    let at = view.page.buttons.len() - 2;
    view.page.buttons[at].label = "Resolve with agent".into();
    view.page.buttons[at + 1].label = "Plan resolution".into();
    view.page.buttons[at + 1].style = ButtonStyle::Quiet;
    if let Some(PageSection::ShipGroup(api)) = view.page.sections.get_mut(1)
        && let Some(card) = api.card.as_mut()
    {
        card.base = Some(tab_line(
            "origin/main has 4 new commits \u{b7} conflicts in src/export.rs, src/routes.rs",
            CheckTone::Blocker,
        ));
    }
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
            made,
            faded: false,
            collapsed: false,
            folds: false,
            stripe,
            field: None,
            field_hint: "",
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
            made: state == "fix made",
            faded,
            collapsed: false,
            folds: false,
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
            super::PageSection::Lines {
                label: "Findings".into(),
                lines: vec![tab_line("2 being fixed", CheckTone::Note)],
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
        primary: None,
        menu: Vec::new(),
        title: "Plan".into(),
        about: Stage::Plan.about().into(),
        status: Some(tab_line("not written yet", CheckTone::Note)),
        buttons: Vec::new(),
        sections: vec![PageSection::Agent {
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
                ("Edit note\u{2026}", quiet),
                ("Pass", plain),
                ("Fail", fail),
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
                "",
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
            made: i == 1,
            faded: tag == FindingTag::Passed,
            collapsed: !open,
            folds: false,
            stripe: None,
            field: writing.then(|| Line::from("Takes 4 seconds")),
            field_hint: "",
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
        text: "1 passed \u{b7} 2 failed \u{b7} 1 untested \u{b7} 1 fix made".into(),
    });
    if note {
        // Placed before the note's Save and Discard join its card, as the
        // page does; the primary stays the page's.
        crate::features::place_buttons(&mut view.page);
        let at = view.page.buttons.len();
        view.page.buttons.extend([
            button("Discard", ButtonStyle::Quiet),
            button("Save", ButtonStyle::Primary),
        ]);
        if let Some(PageSection::Finding(card)) = view.page.sections.last_mut() {
            card.buttons.extend([at, at + 1]);
        }
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
/// open and selected: Make fixes and Plan fixes over them, Add comment….
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
        .insert(3, button("Add comment\u{2026}", ButtonStyle::Plain));
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
        })),
        tag: FindingTag::Failed,
        title: "Make the settings page look better\u{2026}".into(),
        state: "open".into(),
        place: "C1".into(),
        body: String::new(),
        preview: Vec::new(),
        fix: String::new(),
        note: String::new(),
        comment: String::new(),
        comment_buttons: Vec::new(),
        made: false,
        faded: false,
        collapsed: false,
        folds: false,
        stripe: None,
        field: None,
        field_hint: "",
        selected: true,
        buttons: (at..at + 4).collect(),
    }));
    if let Some(strip) = view.test_strip.as_mut() {
        strip.failed = 3;
        strip.text =
            "1 passed \u{b7} 2 failed \u{b7} 1 untested \u{b7} 1 comment open \u{b7} 1 fix made"
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
                asked("AC-4", "Totals in the footer", "", Some(("not tried since", CheckTone::Note))),
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
fn feature_tabs() -> [(&'static str, super::FeatureTabView); 50] {
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
    ];
    views.map(|(name, view)| (name, placed(view)))
}

/// Each fixture at the width it shows best at, and Build at all three.
const TAB_SURFACES: [(&str, i32); 64] = [
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
];

const FEATURE_TAB: [&str; 64] = [
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
];

/// The recorded hashes of `FEATURE_TAB`, per case in `CASES`.
const GOLDEN_FEATURE_TAB: [[u64; 64]; 4] = [
    // Dark at 1
    [
        0xf59c3564f0c82583, // tab_build_860
        0x01a44deb9024d7db, // tab_build_1180
        0x2fa9f178301b98f6, // tab_build_1920
        0x0a078d1fed4d38e8, // tab_plan_ready_1180
        0x368fc43ca45e7aaa, // tab_verify_failed_1920
        0x8133bf04072dc5a9, // tab_verify_running_1180
        0xa47c4e97f5914083, // tab_fix_plan_860
        0x8840c7ebcb481fb5, // tab_ship_blocked_1180
        0x95f439ea2c70cc40, // tab_ship_routes_1180
        0xb4359324290e1108, // tab_ship_routes_860
        0xecd72c65f997dafe, // tab_ship_finish_1180
        0x1038fc3bf524087a, // tab_done_1920
        0x7736e8a70e1faffc, // tab_later_860
        0xd6a0aedff90f1aec, // tab_repos_1180
        0x0fa34cbfc0381e89, // tab_unfolded_860
        0x2382d36c34c898f0, // tab_unfolded_1920
        0x1239d70019e65338, // tab_shipped_1180
        0xb86f3e0c8dde2b73, // tab_repos_failed_1180
        0x710a5a968936fba5, // tab_repos_error_860
        0xdb34483069878a87, // tab_more_repos_850
        0xcfab6f51c5c76d20, // tab_more_repos_mid_850
        0x2708999996e02009, // tab_rail_wrap_500
        0xd5a7cfd2419cd671, // tab_rail_wrap_1180
        0x1fcfd851e2c58650, // tab_picks_later_1180
        0x9c156a9a3e7914f8, // tab_picks_running_860
        0xff1bde061997661e, // tab_model_later_1180
        0xa9bb1bdecdee9042, // tab_model_long_860
        0xcc759a7573a9a0af, // tab_t3_draft_860
        0xc787f0e97371d5eb, // tab_milestone_mode_1180
        0x7926a87a8fc35f6b, // tab_milestone_mode_860
        0x7216ba9ac55db8a2, // tab_held_860
        0xd2338dfb1e8335ec, // tab_resume_1180
        0x990a13e57f472f4c, // tab_resume_860
        0x9fdd4c64193bdeda, // tab_fixing_1180
        0xed2eaa3c0642c7b4, // tab_fixed_code_1180
        0x647af14a47011129, // tab_fixed_code_860
        0xb89d80bae75e906b, // tab_approved_1180
        0xcfa1e2c0ecd3c91c, // tab_locked_1180
        0x37d9cbbb1bc6cdf9, // tab_start_over_1180
        0x23513bc9794d667f, // tab_discover_brief_1180
        0xe42e6757060a6964, // tab_discover_brief_860
        0x488ee2f7493a9a28, // tab_plan_long_1180
        0x47ca89127188b84c, // tab_manual_test_1180
        0x5f07c108da9525f6, // tab_manual_note_860
        0x654a70d538f90e1a, // tab_review_rounds_1180
        0xbd0f9d4ef654cefd, // tab_verify_fixes_1180
        0x24f65baab12064e1, // tab_verify_fixes_860
        0x064cf27ed58e2866, // tab_review_page_1180
        0x3be82ff3cb796e47, // tab_open_round_1180
        0x6df0f1a8011eb41f, // tab_open_round_860
        0xa0f6484816ddbadc, // tab_manual_pages_860
        0xd51edcd58f851a61, // tab_manual_search_860
        0x62eb6b718c110cab, // tab_manual_select_860
        0xc03ab571a4412dd9, // tab_manual_rounds_1180
        0x66612a9849c7d630, // tab_manual_rounds_860
        0x20dee4f22a3e10a3, // tab_manual_comment_860
        0xef5f70aae81e933b, // tab_manual_hand_back_1180
        0x63ed977d1d407115, // tab_ship_live_1180
        0xa1cf36655e878666, // tab_ship_live_860
        0x758b1ac7052370f7, // tab_ship_cards_1180
        0x72b5a4cb5b324c63, // tab_ship_conflicts_860
        0xdd1a5ceec0f55936, // tab_ship_after_1180
        0x86995660db7e46b8, // tab_ai_review_history_1920
        0x03f77adebf2089de, // tab_ai_review_history_860
    ],
    // Dark at 2
    [
        0x2d6aca8fbc7ecea7, // tab_build_860
        0xef45f258ba85c8b4, // tab_build_1180
        0xaec16fd2c43a950a, // tab_build_1920
        0x2cbae6e0739f39d9, // tab_plan_ready_1180
        0xa081c40b4b4ceaa8, // tab_verify_failed_1920
        0xa65137e79116be25, // tab_verify_running_1180
        0xb15c3d134456df13, // tab_fix_plan_860
        0xe0392d8ee4766ae9, // tab_ship_blocked_1180
        0xab8738d29030c95f, // tab_ship_routes_1180
        0x314ea9c8c3a7196a, // tab_ship_routes_860
        0xd9d7e1eb039c8ecd, // tab_ship_finish_1180
        0x33ae295fa4a13207, // tab_done_1920
        0x72bf39899d69bb10, // tab_later_860
        0x701c56b489a2934b, // tab_repos_1180
        0xba22d917d82663e4, // tab_unfolded_860
        0x1236f40b0b645932, // tab_unfolded_1920
        0x743a412973b4cbf2, // tab_shipped_1180
        0x570dbe27de262655, // tab_repos_failed_1180
        0x3130d41139882790, // tab_repos_error_860
        0xc42a7f8e6789b588, // tab_more_repos_850
        0x9ded72870d1ce6bc, // tab_more_repos_mid_850
        0x6c4171a05326f47c, // tab_rail_wrap_500
        0x628eb6cc938388bb, // tab_rail_wrap_1180
        0xb1fef72e53714af1, // tab_picks_later_1180
        0x5c4d9bf04e3e4799, // tab_picks_running_860
        0x3a141f5582b70c94, // tab_model_later_1180
        0x9c71ad3517c111af, // tab_model_long_860
        0x8327f23d0095741e, // tab_t3_draft_860
        0xd38ee5f1e64bf3de, // tab_milestone_mode_1180
        0x7360fb073755c151, // tab_milestone_mode_860
        0xbcdd5f962bdca4d3, // tab_held_860
        0xf7cd2a2e4fe83d2e, // tab_resume_1180
        0xd8d1de1aba62cac3, // tab_resume_860
        0xe3196de0894c4559, // tab_fixing_1180
        0xf63c663c8b76ceff, // tab_fixed_code_1180
        0x8fd577087eb24f5c, // tab_fixed_code_860
        0x9db3b71317ae1033, // tab_approved_1180
        0x09c1cff05c2fe9bb, // tab_locked_1180
        0x490be2cd86816b8f, // tab_start_over_1180
        0x2c77be3601a89c7a, // tab_discover_brief_1180
        0x0f9b0e4f1555fdd4, // tab_discover_brief_860
        0x5eec5c964437f195, // tab_plan_long_1180
        0x94a38044af03a742, // tab_manual_test_1180
        0x3ade3a1fbab932bf, // tab_manual_note_860
        0xcd7873e1c2ae6a09, // tab_review_rounds_1180
        0x236165d61e7bc216, // tab_verify_fixes_1180
        0x0a5f375eb2f25ddf, // tab_verify_fixes_860
        0x7d2c603c8c7abd4b, // tab_review_page_1180
        0x36528fcf691de589, // tab_open_round_1180
        0xef3019975c5ce15a, // tab_open_round_860
        0xf8771ed20382ce3b, // tab_manual_pages_860
        0x8a19dfbf8ecaf937, // tab_manual_search_860
        0xcfc3aea6aedda77d, // tab_manual_select_860
        0x6753708eb4fc6ad8, // tab_manual_rounds_1180
        0x0c8a4145eaf216b8, // tab_manual_rounds_860
        0xd1f3747df54f3c40, // tab_manual_comment_860
        0xe00ae8b51fe74834, // tab_manual_hand_back_1180
        0x059cc0584be1df0f, // tab_ship_live_1180
        0x549787437c447ffd, // tab_ship_live_860
        0xfb9ea8145be25303, // tab_ship_cards_1180
        0x580d897ec5bac8f1, // tab_ship_conflicts_860
        0x71f9eae45118365c, // tab_ship_after_1180
        0x30f6982bcd0641bb, // tab_ai_review_history_1920
        0xe0ea389b36ff3bdd, // tab_ai_review_history_860
    ],
    // Light at 1
    [
        0x7bc1632adf75f504, // tab_build_860
        0xc4c4404bc0f9e352, // tab_build_1180
        0xf3a195e923659bbf, // tab_build_1920
        0xf3c8234f6037cd89, // tab_plan_ready_1180
        0xf2ba559116181a45, // tab_verify_failed_1920
        0x65933052bc86b5ee, // tab_verify_running_1180
        0x151346680d42c79b, // tab_fix_plan_860
        0x399adaeb04c609be, // tab_ship_blocked_1180
        0x78cc4482556111e1, // tab_ship_routes_1180
        0x60815832a1c163bd, // tab_ship_routes_860
        0xfdf00567d55891d2, // tab_ship_finish_1180
        0xf8d82c4dfc5beed0, // tab_done_1920
        0x80b218d3c3578590, // tab_later_860
        0x5d8413c14a346179, // tab_repos_1180
        0xbed2d37b0a881855, // tab_unfolded_860
        0x6195fa540ae77735, // tab_unfolded_1920
        0x5f10885975b9b165, // tab_shipped_1180
        0xeb08d97ea3331476, // tab_repos_failed_1180
        0xdd0983079f77bb50, // tab_repos_error_860
        0x211c3292c855fa22, // tab_more_repos_850
        0x4b84eab60a3e8800, // tab_more_repos_mid_850
        0xd641c5527b5e7a9d, // tab_rail_wrap_500
        0x2d9784015f2b41b3, // tab_rail_wrap_1180
        0x417c4b99ed46fc45, // tab_picks_later_1180
        0xff759a2efcb6e132, // tab_picks_running_860
        0x49daad159aabe893, // tab_model_later_1180
        0x0c1f74c657786aa3, // tab_model_long_860
        0xfd42adba289f128d, // tab_t3_draft_860
        0xc48f7901d8475ff1, // tab_milestone_mode_1180
        0xa5d8f66f8837a37b, // tab_milestone_mode_860
        0x7d8af9da6bd86817, // tab_held_860
        0x7390f158b71fda01, // tab_resume_1180
        0xdea2f6e9152ae2e4, // tab_resume_860
        0x534fabd4b7286173, // tab_fixing_1180
        0x54ff30dff3cdecb8, // tab_fixed_code_1180
        0x84a149bc92c51f21, // tab_fixed_code_860
        0x5666bec634b728ce, // tab_approved_1180
        0x20385a3685245987, // tab_locked_1180
        0x2290243ed3046a14, // tab_start_over_1180
        0xcf95476c44111f84, // tab_discover_brief_1180
        0x5ffda0d58505d23e, // tab_discover_brief_860
        0xed990f364d1f8ed9, // tab_plan_long_1180
        0xb5e80cc609a928cb, // tab_manual_test_1180
        0x4d0f8bdcbd8c2267, // tab_manual_note_860
        0x9199bb487672e7b8, // tab_review_rounds_1180
        0x5a5ee705079932d2, // tab_verify_fixes_1180
        0x651c7a440354ac43, // tab_verify_fixes_860
        0x4a87efc148a68718, // tab_review_page_1180
        0x20e99dcdfdf6c253, // tab_open_round_1180
        0x0d70f9982b14d9d9, // tab_open_round_860
        0x852982f4f61144da, // tab_manual_pages_860
        0xeb41c107be0e46be, // tab_manual_search_860
        0xb071e6f5e564ae1b, // tab_manual_select_860
        0xeba34c1e42605b22, // tab_manual_rounds_1180
        0xc18cb629d9066886, // tab_manual_rounds_860
        0xf51b2846f1949007, // tab_manual_comment_860
        0x21f7f76e89193b64, // tab_manual_hand_back_1180
        0x2b2c121493a9445a, // tab_ship_live_1180
        0x48c48c65b7fcde9c, // tab_ship_live_860
        0x5f4456243b17c3fb, // tab_ship_cards_1180
        0xa90663d10b44dc34, // tab_ship_conflicts_860
        0x23433599d58a2c20, // tab_ship_after_1180
        0x06aa74604bca08ff, // tab_ai_review_history_1920
        0xe43699b1ee8da748, // tab_ai_review_history_860
    ],
    // Light at 2
    [
        0xc49615d3e04361cd, // tab_build_860
        0x1b07bdd50a751634, // tab_build_1180
        0x3e50daef26371b42, // tab_build_1920
        0x6d6c2e0fe4726777, // tab_plan_ready_1180
        0x22787129511504f9, // tab_verify_failed_1920
        0x14a54e53bc26a37b, // tab_verify_running_1180
        0x85a30f2774407220, // tab_fix_plan_860
        0x2dce2a668f55a9cb, // tab_ship_blocked_1180
        0xafb83e1937c6eceb, // tab_ship_routes_1180
        0x6454c2006b381b5f, // tab_ship_routes_860
        0x4183cfc4b636bde0, // tab_ship_finish_1180
        0xe06b101a96692704, // tab_done_1920
        0x37e88e3f815525bd, // tab_later_860
        0x7e270e638a986464, // tab_repos_1180
        0xfcd3179828bdcfe5, // tab_unfolded_860
        0xdf620e131591fc49, // tab_unfolded_1920
        0x38aef6bff62b1904, // tab_shipped_1180
        0x87cc23e2c5a2df7f, // tab_repos_failed_1180
        0x9684215cec35c5b1, // tab_repos_error_860
        0x05a4230993ec44b2, // tab_more_repos_850
        0xd942ddc5c5277d0e, // tab_more_repos_mid_850
        0x302139e0d1e35634, // tab_rail_wrap_500
        0x3d3c83a1fef6fd36, // tab_rail_wrap_1180
        0xd63fc7f72d5b327d, // tab_picks_later_1180
        0xdf9fb93096d4acfa, // tab_picks_running_860
        0x20d19330b6ea3255, // tab_model_later_1180
        0x8eae0ede3db020a3, // tab_model_long_860
        0x0b5b59025292bc90, // tab_t3_draft_860
        0x358b5abd473b0822, // tab_milestone_mode_1180
        0xefb87b30c713f73f, // tab_milestone_mode_860
        0x6255f827336505d2, // tab_held_860
        0x4a71867e3b943735, // tab_resume_1180
        0xc4dd0f7c2b35a5c7, // tab_resume_860
        0x3c6bb75ebfa070a2, // tab_fixing_1180
        0x9bef71a5a982caf1, // tab_fixed_code_1180
        0xc64a0ffb4a46ad3f, // tab_fixed_code_860
        0x5dbc4fb3f6434bbe, // tab_approved_1180
        0x0b145f8421d02901, // tab_locked_1180
        0xcb41b93c53fafe09, // tab_start_over_1180
        0x31a884c0585d1d5b, // tab_discover_brief_1180
        0x5650b96f848f911a, // tab_discover_brief_860
        0xdd7b4e1b42c6de0e, // tab_plan_long_1180
        0x9382e209ca42f83f, // tab_manual_test_1180
        0x68aa015bb4e35b49, // tab_manual_note_860
        0x93149f29f206bd27, // tab_review_rounds_1180
        0x3880f9e87be098d7, // tab_verify_fixes_1180
        0xb8ebffba21c72cd5, // tab_verify_fixes_860
        0xca2398197433012d, // tab_review_page_1180
        0xdc27a92d3004a375, // tab_open_round_1180
        0xdd54be38554baee4, // tab_open_round_860
        0x696190c650513cfc, // tab_manual_pages_860
        0x376359b87d8a2172, // tab_manual_search_860
        0x365f437259b23570, // tab_manual_select_860
        0xb4a155488318ebe0, // tab_manual_rounds_1180
        0xdf2e37532714cfa7, // tab_manual_rounds_860
        0x2c77aba83f813a7b, // tab_manual_comment_860
        0x06cb976c5bd670a3, // tab_manual_hand_back_1180
        0x01cbf011946f9dd9, // tab_ship_live_1180
        0xb22e7a899335f593, // tab_ship_live_860
        0x3e0ef2571cf17463, // tab_ship_cards_1180
        0xce03c04855643b22, // tab_ship_conflicts_860
        0x4da27bec8fcaa12f, // tab_ship_after_1180
        0xa6734e9ca05c8268, // tab_ai_review_history_1920
        0x3834e676458928f8, // tab_ai_review_history_860
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
    use crate::review_cards::{Item, item_buttons, item_card};
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
        let actions = item_buttons(item, true, false);
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
            made: false,
            faded: tag == FindingTag::Skipped,
            collapsed: false,
            folds: false,
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
        0x4f136c4d5a98e009, // tab_findings_1180
        0x9faa24a9dc81d10e, // tab_findings_860
        0xbe15a6efe4a5304a, // finding_dialog
        0x395e21db12f14c4c, // finding_dialog_error
    ],
    // Dark at 2
    [
        0xc6a1b971977a5f50, // tab_findings_1180
        0xf160e2cbc73a9f05, // tab_findings_860
        0x2e982fd65dcbba89, // finding_dialog
        0x1c54a50260a223cf, // finding_dialog_error
    ],
    // Light at 1
    [
        0x3f6e552a11a888cd, // tab_findings_1180
        0xfbf710b8179d3882, // tab_findings_860
        0xc8efa4d7ca6d61b1, // finding_dialog
        0x3068a0f01a434524, // finding_dialog_error
    ],
    // Light at 2
    [
        0xe701bfc3657c230f, // tab_findings_1180
        0x74d733d52ab63dab, // tab_findings_860
        0xf28a5af0ae02e0c9, // finding_dialog
        0x00c403fa84109bce, // finding_dialog_error
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
    use crate::review_cards::{Item, item_buttons, item_card};
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
        let actions = item_buttons(item, !writing, false);
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
        0xfca0eb31cf5758de, // tab_plan_review_cards_1180
        0x0cbedeff9342b2ae, // tab_plan_review_cards_860
        0xcc7ae22863eced67, // tab_plan_review_writing_1180
        0x9afd892c41b157e8, // doc_suggestions_1180
        0xa12094154779b8df, // doc_suggestions_850
        0x6128160606d5890e, // finding_dialog_reject
        0x267d25019293426a, // tab_plan_review_final_1180
    ],
    // Dark at 2
    [
        0x8b9968d99e3aa7ee, // tab_plan_review_cards_1180
        0xebb2eec9ab3dbb7c, // tab_plan_review_cards_860
        0x9c9a1a273ce3fc32, // tab_plan_review_writing_1180
        0xb8dd8bfa55c221fc, // doc_suggestions_1180
        0x26814b5764a0c7e5, // doc_suggestions_850
        0x56d0fa0815bec3ca, // finding_dialog_reject
        0x0bc1eff6f0167bfe, // tab_plan_review_final_1180
    ],
    // Light at 1
    [
        0x0dd1e7d9126bf803, // tab_plan_review_cards_1180
        0x9621d776a38c4f1c, // tab_plan_review_cards_860
        0x900baea579daf213, // tab_plan_review_writing_1180
        0x3f5c1f61a4bd2248, // doc_suggestions_1180
        0xb11d9822b77a1556, // doc_suggestions_850
        0x41c8bd360511db68, // finding_dialog_reject
        0x6fc24c6305870d94, // tab_plan_review_final_1180
    ],
    // Light at 2
    [
        0x3fa5f37a27a07b56, // tab_plan_review_cards_1180
        0x7ed25786b87af0d8, // tab_plan_review_cards_860
        0xdaafd09a48ecb284, // tab_plan_review_writing_1180
        0x991937caceaf8188, // doc_suggestions_1180
        0x0d8e3c393196a9a8, // doc_suggestions_850
        0x036d59753ffc56e9, // finding_dialog_reject
        0x88cfa478b512869c, // tab_plan_review_final_1180
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

fn doc_tabs() -> [(&'static str, super::ReposView, i32); 16] {
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
        tree_share: super::REVIEW_TREE_SHARE,
        grouping: crate::repos::Grouping::Folders,
        lines,
        total,
        selected: Some(3),
        ..findings_popup(2)
    }
}

const DOC_TAB: [&str; 16] = [
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
const GOLDEN_DOC_TAB: [[u64; 16]; 4] = [
    // Dark at 1
    [
        0xd41698c0e3eb8caf, // doc_preview_1180
        0x5b8bbb021fcfe696, // doc_review_1180
        0x89ccc0dbabf24b9e, // doc_review_no_browser
        0x6477071c6d9fee83, // doc_preview_1920
        0x1a7074ec0208a07e, // doc_split_1920
        0x3bf79e5ad23d3c97, // doc_review_850
        0xe7dd74f87bc02f00, // doc_approved_1180
        0x9d24752441fb2e6f, // doc_locked_1180
        0x01ac0d21b0f208c1, // findings_1180
        0x78a3d152b54e92ff, // popup_deciding_1180
        0x605d2d7444c7b1cf, // popup_decided_1180
        0x9e4eb727135dfeba, // popup_code_review_850
        0x6d685c02515d43f0, // popup_no_comments_1180
        0x0b0737987fdde98c, // popup_no_comments_850
        0x657e8f85bed3c689, // popup_listed_1180
        0x9194dd887fc48731, // inline_listed_1180
    ],
    // Dark at 2
    [
        0x0cf6440d43fd3a87, // doc_preview_1180
        0x1b56b6fa51a62cf6, // doc_review_1180
        0xdfe8c036bf434b23, // doc_review_no_browser
        0xb3d7fcb88fa4716a, // doc_preview_1920
        0x24e0cc8828ee1ead, // doc_split_1920
        0x10ac82eeadf5aff3, // doc_review_850
        0x8d9eefb4a2041616, // doc_approved_1180
        0xeff933661765ab3b, // doc_locked_1180
        0xe6c62d0076d5c317, // findings_1180
        0x080afda20215a460, // popup_deciding_1180
        0x98a4d5b3ab0e023d, // popup_decided_1180
        0xa46ae58713877089, // popup_code_review_850
        0xf1a468043c5318c9, // popup_no_comments_1180
        0x1d2f421bf94ab574, // popup_no_comments_850
        0x54807e75c602168d, // popup_listed_1180
        0x982c48687e8278b1, // inline_listed_1180
    ],
    // Light at 1
    [
        0xcd264234eba1810f, // doc_preview_1180
        0x859e4ef58cc8f0cf, // doc_review_1180
        0x2ecc83fda8214b42, // doc_review_no_browser
        0x223a0a300d03a011, // doc_preview_1920
        0x09124e96d542ce6d, // doc_split_1920
        0x4a5824c0a0271033, // doc_review_850
        0x6eaf661db606df03, // doc_approved_1180
        0x5714c6dfe0de599e, // doc_locked_1180
        0x0acb6de136fdf050, // findings_1180
        0xed4cd6ca876f4ccc, // popup_deciding_1180
        0x91b141c5f86d175a, // popup_decided_1180
        0x44082299c48c090a, // popup_code_review_850
        0x515d1c0cad51c048, // popup_no_comments_1180
        0xa8dade21e0ce5f8c, // popup_no_comments_850
        0xbe65ba0bb90a7e79, // popup_listed_1180
        0x0377a1efdec60699, // inline_listed_1180
    ],
    // Light at 2
    [
        0xbd1515957bad3c7b, // doc_preview_1180
        0xf61b3be6e1380db7, // doc_review_1180
        0x60db6706cfdf6ede, // doc_review_no_browser
        0x48731c5d5fd3003e, // doc_preview_1920
        0x33bd6a88f463b002, // doc_split_1920
        0x520823cb7c2526c5, // doc_review_850
        0x83da6eb77b5aa8c1, // doc_approved_1180
        0x6b07e7bfdc861217, // doc_locked_1180
        0xfc151a78713bf308, // findings_1180
        0x6ac5525e742cd634, // popup_deciding_1180
        0xd21f3a24fcf3f5a9, // popup_decided_1180
        0xa61a01d55df7b167, // popup_code_review_850
        0x32ce2ad1b9798180, // popup_no_comments_1180
        0xce9db1c39c7af1f3, // popup_no_comments_850
        0x80b1dfb328209225, // popup_listed_1180
        0x5ddf86739c6faafd, // inline_listed_1180
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
        0x519b4e36dce440ee, // tab_review_code_1180
        0x7ce1d665812bfce7, // tab_review_fixes_1180
        0xc03af1c8522a6d05, // tab_review_fixes_860
        0x94ea673ddef9add5, // tab_review_ready_1180
        0xf1b9c68a649f7789, // tab_review_code_860
        0x3d672a561ce071c4, // tab_review_done_1180
    ],
    // Dark at 2
    [
        0xbdccfff34492a608, // tab_review_code_1180
        0x512f10092713a295, // tab_review_fixes_1180
        0x8cf03168e0c8ccb9, // tab_review_fixes_860
        0xeb89a2967d6df8de, // tab_review_ready_1180
        0x111c2ce2ed7ccec8, // tab_review_code_860
        0x4e2b31bba29b9589, // tab_review_done_1180
    ],
    // Light at 1
    [
        0xc2d55d74f6a99396, // tab_review_code_1180
        0xc149ff46bc27d574, // tab_review_fixes_1180
        0xe83b9f7827f14e1d, // tab_review_fixes_860
        0x7ce2128c81461f4f, // tab_review_ready_1180
        0xe7da46b12d1678b7, // tab_review_code_860
        0x5788bfef059dac13, // tab_review_done_1180
    ],
    // Light at 2
    [
        0x2c3e82f4218e216b, // tab_review_code_1180
        0xb637e7a7268ef8cd, // tab_review_fixes_1180
        0x703e9e99ac27b26f, // tab_review_fixes_860
        0x5c8ee8b8253d1503, // tab_review_ready_1180
        0xd534659e4fd6bd30, // tab_review_code_860
        0x42ec2af0f1bb579f, // tab_review_done_1180
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
        0x920028fc3a703350, // tab_checks_1180
        0xf631ddba32d9b626, // tab_checks_860
    ],
    // Dark at 2
    [
        0x7d5af7e4fc6306a3, // tab_checks_1180
        0xdccdf337cd6f54a6, // tab_checks_860
    ],
    // Light at 1
    [
        0xd23bc5a18f0d9cdf, // tab_checks_1180
        0x3ddc83f00dba67ed, // tab_checks_860
    ],
    // Light at 2
    [
        0x762fb848d5758cbf, // tab_checks_1180
        0xf711ee77f32484a5, // tab_checks_860
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
        0x99966c3b2506d63d, // stats_empty
        0xe0fd68cfb391c1e1, // stats_loaded
        0x3c65cf8dab92ca75, // stats_detail
        0xce6f6560e912d620, // stats_error
        0x964f17e94106c50b, // stats_agents
        0x126ad886e81bab7a, // stats_delete_hover
    ],
    // Dark at 2
    [
        0x70a4fd1990ba17c3, // stats_empty
        0x82796315c0ea2c7e, // stats_loaded
        0x5b2086bde5b455c1, // stats_detail
        0xea2f727ec24cecf2, // stats_error
        0xcd9ce23d4691ec39, // stats_agents
        0x1710f18060c55e46, // stats_delete_hover
    ],
    // Light at 1
    [
        0x7fd4ab28632c785b, // stats_empty
        0x3eded72ab82d41fd, // stats_loaded
        0xd6f7685ff4fec16b, // stats_detail
        0x58f26e96381a3936, // stats_error
        0x175c69b53e482d08, // stats_agents
        0xe662914229435fc0, // stats_delete_hover
    ],
    // Light at 2
    [
        0xe5834abec67891f5, // stats_empty
        0xeb0812fe2bb1f9b2, // stats_loaded
        0x72aa4162c6cdaf13, // stats_detail
        0x2d9933b9ca9b6bb8, // stats_error
        0x0b49475463e76c6b, // stats_agents
        0x76107a0bb1cd1b11, // stats_delete_hover
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
/// keys, a failure being fixed with its note, and folded to its chip.
fn strip_views() -> [super::CriteriaStripView; 5] {
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
        hover: None,
        keys: false,
    };
    let open = CriteriaStripView {
        open: true,
        hover: Some(StripHit::Button(0)),
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
        ..row.clone()
    };
    let failed = CriteriaStripView {
        open: true,
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
    [row, open, writing, failed, chip]
}

const STRIP: [&str; 5] = [
    "strip_row",
    "strip_open",
    "strip_note",
    "strip_failed",
    "strip_chip",
];

fn paint_strip(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    (strip_views().iter())
        .map(|view| hash(painter.criteria_strip(view, scale, p)))
        .collect()
}

/// The recorded hashes of `STRIP`, per case in `CASES`.
const GOLDEN_STRIP: [[u64; 5]; 4] = [
    // Dark at 1
    [
        0x9b9e1214c2a413a5, // strip_row
        0x499a852fd941bec8, // strip_open
        0xf55cfa59cc50a01e, // strip_note
        0xffb86045763e3b31, // strip_failed
        0xe134649e78702bbf, // strip_chip
    ],
    // Dark at 2
    [
        0x4ad958f67e705f77, // strip_row
        0xef8be6f34fac8adf, // strip_open
        0x4ca742e7664fdf25, // strip_note
        0xd486a1f971ba182e, // strip_failed
        0xb63f093b83828479, // strip_chip
    ],
    // Light at 1
    [
        0xeec613f4ff5d62dc, // strip_row
        0xd0f92667606918cf, // strip_open
        0x0765d00a20ba94ad, // strip_note
        0x447a4b6d574a4c8e, // strip_failed
        0x60faa77000796bed, // strip_chip
    ],
    // Light at 2
    [
        0xa66aa28d6e9e00ed, // strip_row
        0xe0d2238f9a5d3a72, // strip_open
        0x850a45e49126971d, // strip_note
        0x43b8569d0a629db5, // strip_failed
        0x45481f7462f5972c, // strip_chip
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
