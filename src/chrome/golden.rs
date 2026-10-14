//! Pixel-identity checks: every chrome surface painted from a fixed view,
//! at scale 1 and 2 in both themes, hashed. The hashes change only when
//! the drawing does; a change made on purpose records the new ones (run
//! the test and copy the table it prints).

use super::{
    AgentLine, AgentView, BarButton, BarStatus, FormField, FormView, Help, ItemIcon, MenuEntry,
    MenuHit, Painter, PaneBarView, ProjectMenuView, ProjectRow, QuitHit, QuitView, Rect,
    SwitcherRow, SwitcherView, TabView, Theme, WelcomeHit, WelcomeView, WorkspaceView,
};
use crate::{WorkspaceKind, git::GitStatus, herdr::AgentStatus, text_field::Line};
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
            }),
            place: "gerdr, gerdr/src".into(),
            activity: "2 tabs · claude working, zsh".into(),
        },
        WorkspaceView {
            name: "~".into(),
            kind: WorkspaceKind::Terminal,
            status: None,
            branch: None,
            git: None,
            place: String::new(),
            activity: String::new(),
        },
    ]
}

fn agents() -> Vec<Vec<AgentView>> {
    vec![
        vec![AgentView {
            pane_id: "p1".into(),
            status: AgentStatus::Blocked,
            kind: "claude".into(),
            name: Some("reviewer".into()),
            lines: vec![
                AgentLine::Title("Reviewing the split".into()),
                AgentLine::Field("cwd".into(), "gerdr/src".into()),
                AgentLine::Field("tokens".into(), "12k".into()),
            ],
        }],
        vec![],
    ]
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
        },
        TabView {
            title: "zsh".into(),
            activity: "cargo watch".into(),
            restart: Some(false),
            ..Default::default()
        },
        TabView::default(),
    ]
}

fn pane_bar() -> PaneBarView {
    PaneBarView {
        rect: Rect::new((0, 0).into(), (600, super::PANE_BAR_HEIGHT).into()),
        label: "npm run dev -- --port 3000".into(),
        note: Some("exited"),
        status: BarStatus::Exited,
        can_restart: true,
        can_stop: false,
        focused: true,
        hover: Some(BarButton::Restart),
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
    }
}

fn switcher() -> SwitcherView {
    let row = |icon, label: &str, detail: &str, hits: &[usize]| SwitcherRow {
        icon,
        label: label.into(),
        detail: detail.into(),
        label_hits: hits.to_vec(),
        detail_hits: Vec::new(),
    };
    SwitcherView {
        query: "ger".into(),
        hint: "Switch to…",
        rows: vec![
            row(
                ItemIcon::Workspace(WorkspaceKind::Project),
                "gerdr",
                "~/Personal/gerdr · main",
                &[0, 1, 2],
            ),
            row(ItemIcon::Tab, "2 zsh", "gerdr", &[]),
            row(
                ItemIcon::Agent(AgentStatus::Working),
                "claude",
                "gerdr",
                &[],
            ),
            row(ItemIcon::Folder, "~/Personal/herdr", "folder", &[]),
            row(ItemIcon::App, "firefox", "firefox http://localhost", &[]),
        ],
        selected: Some(0),
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
        selected: Some(1),
        hover: Some(WelcomeHit::Row(2)),
        naming: None,
        confirm_forget: None,
        note: Some("herdr isn't installed".into()),
    }
}

fn quit() -> QuitView {
    QuitView {
        project: "aft".into(),
        selected: 2,
        dont_ask: true,
        hover: Some(QuitHit::Button(1)),
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
            ("Del".into(), "forget".into()),
            ("Esc".into(), "close".into()),
        ],
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
            error: Some("A project named blog-v2 exists.".into()),
        }),
        settings: None,
        hints: vec![("Esc".into(), "back".into())],
    }
}

const SURFACES: [&str; 11] = [
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
    "notice",
];

/// Every surface at (theme, scale), in `SURFACES` order.
fn paint(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    let help = help();
    let help_layout = painter.help_layout(&help, 800, scale);
    let notice = "Switched to gerdr · aft keeps running";
    let notice_w = painter.notice_width(notice, 600, scale);
    let strip = (900, super::TAB_STRIP_HEIGHT).into();
    vec![
        hash(painter.sidebar(
            &workspaces(),
            &agents(),
            Some(0),
            false,
            (Some("aft"), true),
            (200, 400).into(),
            scale,
            p,
        )),
        hash(painter.tab_strip(&tabs(), 1, true, theme, strip, scale)),
        hash(painter.pane_bar(&pane_bar(), scale, p)),
        hash(painter.help_panel(&help, &help_layout, scale, p)),
        hash(painter.menu(Some(0), scale, p)),
        hash(painter.switcher(&switcher(), 640, scale, p)),
        hash(painter.welcome(&welcome(), (800, 600).into(), scale, p)),
        hash(painter.quit_dialog(&quit(), (800, 600).into(), scale, p)),
        hash(painter.project_menu(&project_menu(), (800, 600).into(), scale, p)),
        hash(painter.project_menu(&form(), (800, 600).into(), scale, p)),
        hash(painter.notice(notice, notice_w, scale, p)),
    ]
}

const CASES: [(Theme, f64); 4] = [
    (Theme::Dark, 1.0),
    (Theme::Dark, 2.0),
    (Theme::Light, 1.0),
    (Theme::Light, 2.0),
];

/// The recorded hashes, per case in `CASES`, per surface in `SURFACES`.
const GOLDEN: [[u64; 11]; 4] = [
    // Dark at 1
    [
        0x43ca054776e2dca6, // sidebar
        0x9dab172b740847be, // tab_strip
        0xef4cb65ed4fb8d88, // pane_bar
        0x22a8ad5e9d782269, // help
        0xeac2e664137d8223, // menu
        0xc89633ad106f2545, // switcher
        0x875ceb1dfa7a9d4e, // welcome
        0x59378f4fb455dd74, // quit
        0xbdf19f0d16102844, // project_menu
        0xf752833083205073, // project_form
        0x72d2762f8cc49297, // notice
    ],
    // Dark at 2
    [
        0x72304d72f707aff2, // sidebar
        0x661750a229288705, // tab_strip
        0x281c7b1265db6d9a, // pane_bar
        0xf98f07251f79c2f7, // help
        0x1483bdc67e595289, // menu
        0x3d5ed50b48a63b89, // switcher
        0x92cc71d8edd24f6e, // welcome
        0xca6aac6e936af23a, // quit
        0x26b986af5a54ece3, // project_menu
        0x10f6610ed411e0f6, // project_form
        0x11bb51a4e99521d5, // notice
    ],
    // Light at 1
    [
        0x94d741996e595b02, // sidebar
        0xfc4462d17f57d61c, // tab_strip
        0x99008e3a97bea6eb, // pane_bar
        0x636e336a63c10923, // help
        0x3276e8655270d746, // menu
        0x835a78ec2096c7e8, // switcher
        0x125d806a2bf41bd7, // welcome
        0x9141f1eb6dfa4384, // quit
        0xf631d81fb6f3de45, // project_menu
        0xbd0cf341186b13f3, // project_form
        0x2e8be3bb240be7f4, // notice
    ],
    // Light at 2
    [
        0x0e382704f9883d18, // sidebar
        0x7c5f011dd31a0daf, // tab_strip
        0xab864158c8d25885, // pane_bar
        0x8245be479f2e2165, // help
        0x129626ffb502a663, // menu
        0x7b2411445874e6e1, // switcher
        0xcf5df50d9a47fd2b, // welcome
        0xea36deb588723e15, // quit
        0xc05659f1d0615c03, // project_menu
        0x7a8b78a88a042b46, // project_form
        0x81b629b422e8830b, // notice
    ],
];

#[test]
fn surfaces_paint_the_same_pixels() {
    let painter = Painter::new();
    assert!(painter.font.is_some(), "the golden hashes need a font");
    let mut table = String::new();
    let mut same = true;
    for ((theme, scale), want) in CASES.into_iter().zip(GOLDEN) {
        let got = paint(&painter, theme, scale);
        table += &format!("        // {theme:?} at {scale}\n        [\n");
        for (i, (surface, hash)) in SURFACES.iter().zip(&got).enumerate() {
            if *hash != want[i] {
                same = false;
                eprintln!(
                    "{surface} ({theme:?}, {scale}): {hash:#018x}, recorded {:#018x}",
                    want[i]
                );
            }
            table += &format!("            {hash:#018x}, // {surface}\n");
        }
        table += "        ],\n";
    }
    assert!(
        same,
        "the chrome paints differently; if on purpose, record:\n{table}"
    );
}
