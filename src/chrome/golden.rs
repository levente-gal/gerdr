//! Pixel-identity checks: every chrome surface painted from a fixed view,
//! at scale 1 and 2 in both themes, hashed. The hashes change only when
//! the drawing does; a change made on purpose records the new ones (run
//! the test and copy the table it prints).

use std::time::Instant;

use super::{
    AgentLine, AgentView, BarButton, BarStatus, FormField, FormView, Help, ItemIcon, MenuEntry,
    MenuHit, MenuRow, MenuView, Painter, PaneBarView, PlaceholderView, ProjectMenuView, ProjectRow,
    QuitHit, QuitView, Rect, SwitcherRow, SwitcherView, TabView, Theme, ToastView, TooltipView,
    WelcomeHit, WelcomeView, WorkspaceView,
};
use crate::{WorkspaceKind, git::GitStatus, herdr::AgentStatus, text_field::Line, toast::Level};
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
    }
}

fn tooltip() -> TooltipView {
    TooltipView {
        text: "Restart what runs here".into(),
        key: Some("Ctrl+B a".into()),
        anchor: Rect::new((300, 48).into(), (24, 24).into()),
    }
}

fn placeholder(timed_out: bool) -> PlaceholderView {
    PlaceholderView {
        rect: Rect::new((200, 48).into(), (480, 320).into()),
        label: "Zed".into(),
        since: Instant::now(),
        timed_out,
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
    "toast",
];

/// Every surface at (theme, scale), in `SURFACES` order.
fn paint(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    let help = help();
    let help_layout = painter.help_layout(&help, 800, false, scale);
    let notice = toast(None, Level::Info);
    let strip = (900, super::tab_strip_height(super::Density::Comfortable)).into();
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
        hash(painter.menu(&menu(), scale, p)),
        hash(painter.switcher(&switcher(), 640, scale, p)),
        hash(painter.welcome(&welcome(), (800, 600).into(), scale, p)),
        hash(painter.quit_dialog(&quit(), (800, 600).into(), scale, p)),
        hash(painter.project_menu(&project_menu(), (800, 600).into(), scale, p)),
        hash(painter.project_menu(&form(), (800, 600).into(), scale, p)),
        hash(painter.toast(&notice, 360, false, scale, p)),
    ]
}

/// The surfaces the motion work added, apart so the table above keeps
/// its shape: a menu with hints, a pinned help panel, a toast with a
/// button, a tooltip, the placeholder waiting and timed out, the spinner.
const OVERLAYS: [&str; 7] = [
    "full_menu",
    "help_pinned",
    "toast_action",
    "tooltip",
    "placeholder",
    "placeholder_timed_out",
    "spinner",
];

fn paint_overlays(painter: &Painter, theme: Theme, scale: f64) -> Vec<u64> {
    let p = theme.palette();
    let help = help();
    let pinned = painter.help_layout(&help, 800, true, scale);
    let tip = tooltip();
    let tip_w = painter.tooltip_width(&tip, 400, scale);
    vec![
        hash(painter.menu(&full_menu(), scale, p)),
        hash(painter.help_panel(&help, &pinned, scale, p)),
        hash(painter.toast(&toast(Some("Retry"), Level::Error), 360, true, scale, p)),
        hash(painter.tooltip(&tip, tip_w, scale, p)),
        hash(painter.placeholder(&placeholder(false), false, scale, p)),
        hash(painter.placeholder(&placeholder(true), true, scale, p)),
        hash(painter.spinner(5, scale, p)),
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
        0x67589784b798d683, // sidebar
        0x13e27886dba77bed, // tab_strip
        0xbbe172db050ef7d7, // pane_bar
        0x866d59a70ebe252d, // help
        0xeac2e664137d8223, // menu
        0x517d325aca9d2875, // switcher
        0x2339adcf1ef00249, // welcome
        0x59378f4fb455dd74, // quit
        0xdf352140e5a10a7e, // project_menu
        0xb5de34a9d82e1c98, // project_form
        0xa135931fecb12b15, // toast
    ],
    // Dark at 2
    [
        0xc564686cbb8d9f53, // sidebar
        0x10cad43cd88fc070, // tab_strip
        0x68ae94c90fb4c936, // pane_bar
        0x56045b72862ed173, // help
        0x1483bdc67e595289, // menu
        0x505dae5090ad952a, // switcher
        0xe0573455650e0f53, // welcome
        0xca6aac6e936af23a, // quit
        0x2c0d505acdca739a, // project_menu
        0xfebfa0e09839ef5a, // project_form
        0x03fe8c8c1a28bb89, // toast
    ],
    // Light at 1
    [
        0x05f69fc665a353b7, // sidebar
        0x13d9d07a680d567f, // tab_strip
        0x6028c8d59ce91601, // pane_bar
        0x8c0302567b35f0c3, // help
        0x3276e8655270d746, // menu
        0xe323c364f69d5042, // switcher
        0x50c5d783b4da69ca, // welcome
        0x9141f1eb6dfa4384, // quit
        0xa02bd03173402f8c, // project_menu
        0x7099ffbd466ef3ad, // project_form
        0x09527b9387445087, // toast
    ],
    // Light at 2
    [
        0x07d08fe1e4e286dc, // sidebar
        0xaebc10244cf8b919, // tab_strip
        0xd1b4f4a4a80352ee, // pane_bar
        0x22f444d49649f855, // help
        0x129626ffb502a663, // menu
        0x1b96323b30d65bb4, // switcher
        0x195335c49651f7c6, // welcome
        0xea36deb588723e15, // quit
        0xc2172f68d48557aa, // project_menu
        0x6667f8cc8b4a1461, // project_form
        0x3a08c772671fb90b, // toast
    ],
];

/// The recorded hashes of `OVERLAYS`, per case in `CASES`.
const GOLDEN_OVERLAYS: [[u64; 7]; 4] = [
    // Dark at 1
    [
        0xa2b92bfd04e8baf2, // full_menu
        0x833a9487b45a04b1, // help_pinned
        0x1be09a92a642789d, // toast_action
        0x08b20795a8acaa71, // tooltip
        0xd5b2438c44e2d360, // placeholder
        0x11543d384eae786f, // placeholder_timed_out
        0x7a426c6be23d66a6, // spinner
    ],
    // Dark at 2
    [
        0xc4863a27ee827b7f, // full_menu
        0xa52ac88120332539, // help_pinned
        0xc1ea1c0802cb326e, // toast_action
        0x78986509fec8138c, // tooltip
        0x94bf628b2c8d8e61, // placeholder
        0xe03b73375f1bb153, // placeholder_timed_out
        0x2f28c94a7002e520, // spinner
    ],
    // Light at 1
    [
        0x3bc1ce012f5b7d01, // full_menu
        0x50daf004ca030b63, // help_pinned
        0x0482566dca739991, // toast_action
        0x59892211d7643278, // tooltip
        0x848913a059883552, // placeholder
        0x0e2d07d0ca3988e3, // placeholder_timed_out
        0x4345ed79149d747b, // spinner
    ],
    // Light at 2
    [
        0x3ab6da8086162a68, // full_menu
        0x357c8c2ee4c38806, // help_pinned
        0xc47e8d329efbe42a, // toast_action
        0xc0b4132b818d96b8, // tooltip
        0x9bc50d6953da1fd5, // placeholder
        0x7a29e3abd1973978, // placeholder_timed_out
        0x883035c0209e314f, // spinner
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
        for px in mem.chunks_exact(4) {
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
            painter.help_panel(&help, &pinned, scale, p),
            px(pinned.size.w + shadow),
        );
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
            &format!("tooltip_{tag}"),
            painter.tooltip(&tip, tip_w, scale, p),
            px(tip_w + shadow),
        );
        dump(
            &dir,
            &format!("placeholder_{tag}"),
            painter.placeholder(&waiting, false, scale, p),
            px(waiting.rect.size.w),
        );
        dump(
            &dir,
            &format!("placeholder_late_{tag}"),
            painter.placeholder(&placeholder(true), true, scale, p),
            px(waiting.rect.size.w),
        );
        dump(
            &dir,
            &format!("spinner_{tag}"),
            painter.spinner(5, scale, p),
            px(super::SPINNER_SIZE),
        );
    }
}
