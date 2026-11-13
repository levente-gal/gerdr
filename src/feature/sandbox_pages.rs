//! The pages of a sandbox's app: what Open app and the buttons beside it open.
//!
//! The project's settings can list them (`workflow.sandbox_pages`, the first
//! being the one Open app opens), as the project commands dialog's Pages tab
//! edits them (`to_lines`, `from_lines`). Without, they come from the commands the
//! sandbox runs: a Django server gives its site, then its admin when it has
//! one; Expo gives its web client, which never comes first, as the backend's
//! own site is what is tried by hand. A JavaScript dev server (Vite, Nx, `npm
//! run dev`) comes before them all: it is the app, Django its API then. Its port
//! is guessed from what listens as it opens (`sandbox_run::app_port`), as Open
//! app does with no pages at all. A command running a shell script is read
//! through it.
//!
//! They are found as a sandbox starts and kept in its record, so a gerdr on
//! another machine shows the same buttons.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Django's own port, when `runserver` is given none.
const DJANGO_PORT: u16 = 8000;
/// Metro's, which serves Expo's web client too.
const EXPO_PORT: u16 = 8081;
/// How deep a repository is searched for its `urls.py` files.
const URLS_DEPTH: usize = 4;

/// A page of a sandbox's app.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    /// What its button says.
    pub name: String,
    /// Where it is, on the sandbox's `localhost`.
    pub url: String,
    /// Its port is what looks most like an app of what listens as it opens,
    /// past the other pages' ports; `url` says no port then.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub guess: bool,
}

impl Page {
    fn new(name: &str, port: u16, path: &str) -> Self {
        Page {
            name: name.to_owned(),
            url: format!("http://localhost:{port}{path}"),
            guess: false,
        }
    }

    /// The page of a dev server whose port only listening tells.
    fn guessed(name: &str) -> Self {
        Page {
            name: name.to_owned(),
            url: "http://localhost/".to_owned(),
            guess: true,
        }
    }
}

/// A command the sandbox runs, in the folder it runs in.
#[derive(Clone, Copy)]
pub struct Ran<'a> {
    pub dir: &'a Path,
    pub command: &'a str,
}

/// The pages of a sandbox running `ran`: those `set` lists, else the one `url`
/// names, else those its commands tell of. Empty when nothing tells.
pub fn pages(set: &[Page], url: Option<&str>, ran: &[Ran]) -> Vec<Page> {
    if !set.is_empty() {
        return set.to_vec();
    }
    if let Some(url) = url {
        return vec![Page {
            name: "App".to_owned(),
            url: url.to_owned(),
            guess: false,
        }];
    }
    let mut first = Vec::new();
    let mut last = Vec::new();
    let mut app = false;
    for ran in ran {
        let script = shell_script(ran.command, ran.dir).unwrap_or_default();
        let lines = (script.lines()).filter(|line| !line.trim_start().starts_with('#'));
        for command in std::iter::once(ran.command).chain(lines) {
            if let Some(port) = django_port(command) {
                first.push(Page::new("Web app", port, "/"));
                if let Some(path) = admin_path(ran.dir) {
                    first.push(Page::new("Django admin", port, &path));
                }
            } else if let Some(port) = expo_port(command, ran.dir)
                && has_web(ran.dir)
            {
                last.push(Page::new("Expo web", port, "/"));
            } else if js_dev_server(command, ran.dir) {
                app = true;
            }
        }
    }
    if app {
        first.insert(0, Page::guessed("App"));
    }
    first.extend(last);
    let mut seen = Vec::new();
    first.retain(|page| {
        let new = !seen.contains(&page.url);
        seen.push(page.url.clone());
        new
    });
    first
}

/// The pages of a sandbox whose commands told of `guessed`: those `set`
/// lists, else the one `url` names, else `guessed`.
pub fn settled(set: &[Page], url: Option<&str>, guessed: &[Page]) -> Vec<Page> {
    match pages(set, url, &[]) {
        pages if pages.is_empty() => guessed.to_vec(),
        pages => pages,
    }
}

/// The text of the shell script `command`, run in `dir`, runs: `bash
/// ./runlocal.sh`, `sh run.sh`, `./run.sh`. Its servers are started inside,
/// out of the command's sight.
fn shell_script(command: &str, dir: &Path) -> Option<String> {
    let words = words(command);
    let mut rest = words.iter().copied().skip_while(|w| w.contains('='));
    let first = rest.next()?;
    let shell = Path::new(first).file_name()?.to_str()?;
    let file = if matches!(shell, "bash" | "sh" | "zsh" | "dash") {
        rest.find(|w| !w.starts_with('-'))?
    } else if first.ends_with(".sh") {
        first
    } else {
        return None;
    };
    let path = dir.join(file);
    // A script is small; anything big is no script to read through.
    if fs::metadata(&path).ok()?.len() > 64 * 1024 {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// Whether `command`, run in `dir`, starts a JavaScript dev server: itself, or
/// through a script of `dir`'s `package.json`.
fn js_dev_server(command: &str, dir: &Path) -> bool {
    let starts = |words: &[&str]| {
        let tool = |w: &str| {
            Path::new(w)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(w)
                .to_owned()
        };
        let names: Vec<String> = words.iter().map(|w| tool(w)).collect();
        let has = |name: &str| names.iter().any(|n| n == name);
        let then = |name: &str, next: &[&str]| {
            (names.windows(2)).any(|pair| pair[0] == name && next.contains(&pair[1].as_str()))
        };
        has("vite")
            || has("webpack-dev-server")
            || then("webpack", &["serve"])
            || then("ng", &["serve"])
            || then("nx", &["serve"])
            || (has("nx") && names.iter().any(|n| n.contains(":serve")))
            || then("next", &["dev"])
            || then("nuxt", &["dev"])
            || then("astro", &["dev"])
            || then("react-scripts", &["start"])
    };
    let words = words(command);
    if starts(&words) {
        return true;
    }
    let Some(script) = script_name(&words) else {
        return false;
    };
    (read_manifest(dir).as_ref())
        .and_then(|m| m.get("scripts")?.get(script)?.as_str().map(str::to_owned))
        .is_some_and(|text| starts(&self::words(&text)))
}

/// The port a Django server `command` starts listens on, if it starts one.
fn django_port(command: &str) -> Option<u16> {
    let words = words(command);
    let at = words.iter().position(|w| *w == "runserver")?;
    let django = words[..at]
        .iter()
        .any(|w| w.ends_with("manage.py") || w.ends_with("django-admin"));
    if !django {
        return None;
    }
    let given = words[at + 1..].iter().find(|w| !w.starts_with('-'));
    Some(given.and_then(|w| addr_port(w)).unwrap_or(DJANGO_PORT))
}

/// The port of `addr`, as `runserver` takes it: `9001`, `0.0.0.0:9001`,
/// `[::]:9001`.
fn addr_port(addr: &str) -> Option<u16> {
    let port = addr.rsplit_once(':').map_or(addr, |(_, port)| port);
    port.parse().ok()
}

/// The port Metro listens on, if `command`, run in `dir`, starts Expo: itself,
/// or through a script of `dir`'s `package.json` that does.
fn expo_port(command: &str, dir: &Path) -> Option<u16> {
    let words = words(command);
    let starts = |words: &[&str]| {
        (words.windows(2)).any(|pair| pair[0].ends_with("expo") && pair[1] == "start")
    };
    if starts(&words) {
        return Some(port_flag(&words).unwrap_or(EXPO_PORT));
    }
    let script = script_name(&words)?;
    let manifest = read_manifest(dir)?;
    let text = manifest.get("scripts")?.get(script)?.as_str()?;
    let theirs = self::words(text);
    starts(&theirs).then(|| {
        port_flag(&words)
            .or(port_flag(&theirs))
            .unwrap_or(EXPO_PORT)
    })
}

/// The script `words` run through a package manager: `npm start`, `npm run
/// web`, `yarn start`, `pnpm run start`, `bun run start`.
fn script_name<'a>(words: &[&'a str]) -> Option<&'a str> {
    let at = words
        .iter()
        .position(|w| matches!(*w, "npm" | "yarn" | "pnpm" | "bun"))?;
    let mut rest = words[at + 1..]
        .iter()
        .copied()
        .filter(|w| !w.starts_with('-'));
    match rest.next()? {
        "run" | "run-script" => rest.next(),
        "start" => Some("start"),
        // yarn and pnpm run a script named first.
        other if words[at] != "npm" => Some(other),
        _ => None,
    }
}

/// The port `--port N`, `--port=N` or `-p N` gives in `words`.
fn port_flag(words: &[&str]) -> Option<u16> {
    let mut words = words.iter();
    while let Some(word) = words.next() {
        if let Some(port) = word.strip_prefix("--port=") {
            return port.parse().ok();
        }
        if matches!(*word, "--port" | "-p") {
            return words.next()?.parse().ok();
        }
    }
    None
}

/// Whether the Expo app in `dir` has a web client: React Native for Web in its
/// dependencies.
fn has_web(dir: &Path) -> bool {
    let Some(manifest) = read_manifest(dir) else {
        return false;
    };
    ["dependencies", "devDependencies"].iter().any(|key| {
        manifest
            .get(key)
            .and_then(|d| d.get("react-native-web"))
            .is_some()
    })
}

/// `dir`'s `package.json`, when it reads.
fn read_manifest(dir: &Path) -> Option<serde_json::Value> {
    let path = dir.join("package.json");
    let text = fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(why) => {
            tracing::warn!("{}: {why}", path.display());
            None
        }
    }
}

/// The path of the Django admin in the project in `dir`, as its `urls.py`
/// files route to `admin.site.urls`.
fn admin_path(dir: &Path) -> Option<String> {
    let mut files = Vec::new();
    urls_files(dir, URLS_DEPTH, &mut files);
    files
        .iter()
        .filter_map(|file| fs::read_to_string(file).ok())
        .find_map(|text| admin_route(&text))
}

/// The `urls.py` files under `dir`, `depth` folders down, past what holds no
/// code of the project's own.
fn urls_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            let skip = name.starts_with('.')
                || matches!(
                    &*name,
                    "node_modules" | "venv" | "env" | "site-packages" | "static"
                );
            if depth > 0 && !skip {
                urls_files(&path, depth - 1, out);
            }
        } else if name == "urls.py" {
            out.push(path);
        }
    }
}

/// The route `text`, a `urls.py`, gives the admin: `path("admin/",
/// admin.site.urls)` is `/admin/`.
fn admin_route(text: &str) -> Option<String> {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter(|line| line.contains("admin.site.urls"))
        .find_map(|line| {
            let open = line.find(['"', '\''])?;
            let quote = line[open..].chars().next()?;
            let rest = &line[open + 1..];
            let route = &rest[..rest.find(quote)?];
            // A regex route, as `url(r'^admin/', ...)` has.
            let route = route.trim_start_matches('^').trim_end_matches('$');
            Some(format!("/{}", route.trim_start_matches('/')))
        })
}

/// `command`'s words, past variables set before it.
fn words(command: &str) -> Vec<&str> {
    command.split_whitespace().collect()
}

/// `pages` as the project commands dialog shows them: a line each, its name
/// then its address. A guessed page's address says no port.
pub fn to_lines(pages: &[Page]) -> Vec<String> {
    (pages.iter())
        .map(|page| format!("{} {}", page.name, page.url))
        .collect()
}

/// The pages `text` lists, a line each as `to_lines` writes them: a name then
/// an address. A line of only an address is named for it, the first "App". A
/// `localhost` address with no port is guessed from what listens. Else why
/// not, as a sentence.
pub fn from_lines(text: &str) -> Result<Vec<Page>, String> {
    let mut pages = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let words = words(line);
        let url = words[words.len() - 1];
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(format!(
                "\u{201c}{line}\u{201d} has no address. A page is its name, then an address such as http://localhost:5173/."
            ));
        }
        let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
        // With its port, if it says one.
        let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
        let name = match words[..words.len() - 1].join(" ") {
            name if !name.is_empty() => name,
            _ if pages.is_empty() => "App".to_owned(),
            _ => host.to_owned(),
        };
        pages.push(Page {
            name,
            url: url.to_owned(),
            guess: host == "localhost",
        });
    }
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gerdr-pages-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn django_gives_its_site_then_its_admin_and_expo_comes_last() {
        let api = scratch("api");
        fs::create_dir_all(api.join("Asset-Track")).unwrap();
        fs::write(
            api.join("Asset-Track/urls.py"),
            "urlpatterns = [\n    # path('old/', admin.site.urls),\n    path(\"admin/\", admin.site.urls),\n]\n",
        )
        .unwrap();
        let mobile = scratch("mobile");
        fs::write(
            mobile.join("package.json"),
            r#"{"scripts":{"start":"expo start"},"dependencies":{"react-native-web":"^0.21"}}"#,
        )
        .unwrap();
        let ran = [
            Ran {
                dir: &mobile,
                command: "npm start",
            },
            Ran {
                dir: &api,
                command: "python manage.py runserver 0.0.0.0:9001",
            },
            Ran {
                dir: &api,
                command: "python manage.py qcluster",
            },
        ];
        let urls: Vec<(String, String)> = pages(&[], None, &ran)
            .into_iter()
            .map(|p| (p.name, p.url))
            .collect();
        assert_eq!(
            urls,
            [
                ("Web app".to_owned(), "http://localhost:9001/".to_owned()),
                (
                    "Django admin".to_owned(),
                    "http://localhost:9001/admin/".to_owned()
                ),
                ("Expo web".to_owned(), "http://localhost:8081/".to_owned()),
            ]
        );
    }

    #[test]
    fn servers_a_shell_script_starts_give_pages() {
        let api = scratch("script");
        fs::write(
            api.join("urls.py"),
            "urlpatterns = [path('admin/', admin.site.urls)]\n",
        )
        .unwrap();
        fs::write(
            api.join("runlocal.sh"),
            "#!/bin/bash\n\nsource .venv/bin/activate\n\npython manage.py runserver 0.0.0.0:9000\n# python manage.py runserver 9999\n",
        )
        .unwrap();
        let web = [
            Page::new("Web app", 9000, "/"),
            Page::new("Django admin", 9000, "/admin/"),
        ];
        for command in [
            "/bin/bash ./runlocal.sh",
            "sh -e runlocal.sh",
            "./runlocal.sh",
            "X=1 bash runlocal.sh",
        ] {
            let ran = [Ran { dir: &api, command }];
            assert_eq!(pages(&[], None, &ran), web, "{command}");
        }
        let ran = [Ran {
            dir: &api,
            command: "bash ./missing.sh",
        }];
        assert_eq!(pages(&[], None, &ran), []);
    }

    #[test]
    fn a_js_dev_server_is_the_app_before_django() {
        let api = scratch("js-api");
        fs::write(
            api.join("urls.py"),
            "urlpatterns = [path('admin/', admin.site.urls)]\n",
        )
        .unwrap();
        let web = scratch("js-web");
        fs::write(
            web.join("package.json"),
            r#"{"scripts":{"dev":"vite --host"}}"#,
        )
        .unwrap();
        let django = Ran {
            dir: &api,
            command: "python manage.py runserver 0.0.0.0:9000",
        };
        let after = [
            Page::new("Web app", 9000, "/"),
            Page::new("Django admin", 9000, "/admin/"),
        ];
        for command in [
            "npx nx run ao-app:serve:local",
            "npm run dev",
            "npx vite",
            "ng serve --port 4300",
            "npx next dev",
        ] {
            let ran = [django, Ran { dir: &web, command }];
            let mut want = vec![Page::guessed("App")];
            want.extend(after.clone());
            assert_eq!(pages(&[], None, &ran), want, "{command}");
        }
        for command in ["npm run lint", "npx nx run ao-app:test", "node server.js"] {
            let ran = [django, Ran { dir: &web, command }];
            assert_eq!(pages(&[], None, &ran), after, "{command}");
        }
    }

    #[test]
    fn pages_are_written_and_read_a_line_each() {
        let pages = [
            Page::guessed("App"),
            Page::new("Django admin", 8000, "/admin/"),
        ];
        let lines = to_lines(&pages);
        assert_eq!(
            lines,
            [
                "App http://localhost/",
                "Django admin http://localhost:8000/admin/"
            ]
        );
        assert_eq!(from_lines(&lines.join("\n")).unwrap(), pages);
        // Unnamed, the first is the app and the rest are named for their host.
        let read = from_lines("\n  http://localhost:4200/ \nhttps://api.test:8443/docs\n").unwrap();
        assert_eq!(read[0].name, "App");
        assert!(!read[0].guess);
        assert_eq!(read[1].name, "api.test:8443");
        assert_eq!(from_lines("").unwrap(), []);
        assert!(from_lines("App localhost:4200").is_err());
        assert!(from_lines("App").is_err());
    }

    #[test]
    fn settings_come_before_what_commands_tell() {
        let set = [Page {
            name: "Home".into(),
            url: "http://localhost:3/".into(),
            guess: false,
        }];
        let dir = scratch("set");
        let ran = [Ran {
            dir: &dir,
            command: "./manage.py runserver",
        }];
        assert_eq!(pages(&set, Some("http://localhost:4"), &ran), set);
        let one = pages(&[], Some("http://localhost:4"), &ran);
        assert_eq!(
            one,
            [Page {
                name: "App".into(),
                url: "http://localhost:4".into(),
                guess: false,
            }]
        );
        assert_eq!(pages(&[], None, &ran), [Page::new("Web app", 8000, "/")]);
        // What the commands told of is kept for when the settings are emptied.
        let guessed = [Page::guessed("App")];
        assert_eq!(settled(&set, None, &guessed), set);
        assert_eq!(settled(&[], Some("http://localhost:4"), &guessed), one);
        assert_eq!(settled(&[], None, &guessed), guessed);
    }

    #[test]
    fn expo_without_a_web_client_has_no_page() {
        let dir = scratch("noweb");
        fs::write(
            dir.join("package.json"),
            r#"{"dependencies":{"expo":"57"}}"#,
        )
        .unwrap();
        let ran = [Ran {
            dir: &dir,
            command: "npx expo start --port 19000",
        }];
        assert!(pages(&[], None, &ran).is_empty());
        fs::write(
            dir.join("package.json"),
            r#"{"devDependencies":{"react-native-web":"1"}}"#,
        )
        .unwrap();
        assert_eq!(pages(&[], None, &ran), [Page::new("Expo web", 19000, "/")]);
    }

    #[test]
    fn ports_are_read_as_each_tool_takes_them() {
        assert_eq!(django_port("python manage.py runserver"), Some(8000));
        assert_eq!(
            django_port("uv run ./manage.py runserver --noreload [::]:8002"),
            Some(8002)
        );
        assert_eq!(django_port("django-admin runserver 9"), Some(9));
        assert_eq!(django_port("python manage.py qcluster"), None);
        assert_eq!(django_port("runserver 9"), None);
        let none = Path::new("/nonexistent");
        assert_eq!(expo_port("npx expo start --port=8082", none), Some(8082));
        assert_eq!(expo_port("expo start -p 1", none), Some(1));
        assert_eq!(expo_port("npm test", none), None);
        assert_eq!(script_name(&["npm", "run", "web"]), Some("web"));
        assert_eq!(script_name(&["yarn", "web"]), Some("web"));
        assert_eq!(script_name(&["npm", "test"]), None);
    }

    #[test]
    fn the_admin_route_is_read_from_its_line() {
        assert_eq!(
            admin_route("path('backoffice/', admin.site.urls),"),
            Some("/backoffice/".into())
        );
        assert_eq!(
            admin_route("url(r'^admin/', admin.site.urls),"),
            Some("/admin/".into())
        );
        assert_eq!(admin_route("path('a/', include(x)),"), None);
    }
}
