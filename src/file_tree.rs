//! The lines of a list of changed files, one or more repositories' worth:
//! a line each with its whole path, or under a line per folder, folders
//! folding. The repositories panel's tree has them, and so has a feature's
//! code review (see `FileTree`).

use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

use smithay::input::keyboard::{Keysym, ModifiersState};

use crate::{
    chrome::{Check, Density, DirLine, FileLine, LineState, RepoLine, ReposView, TreeLine},
    diff_view::{CheckOf, DiffView, Moved, Place, Section, bounds, in_dir, move_to, showing},
    git::Change,
    repos::{Grouping, Selected, standing},
    scroll::{Bounds, Scroll},
};

/// A tree line, by index: a repository of `Ctx::items`, a folder of its
/// changes, or one of its changes, by index into its `GitStatus::changes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    Repo(usize),
    /// A folder of repository `k`, `depth` folders in: the first `to`
    /// parts of the path of change `c` (the first file under it), shown
    /// as its parts `from..to`, as a folder holding nothing but one folder
    /// is one line with it.
    Dir {
        k: usize,
        c: usize,
        from: usize,
        to: usize,
        depth: usize,
    },
    /// Change `c` of repository `k`, `depth` folders in.
    File(usize, usize, usize),
    /// The heading over the repositories with nothing to do.
    Clean,
}

impl Row {
    /// The repository it is, or is under; none for the heading.
    pub(crate) fn item(self) -> Option<usize> {
        match self {
            Row::Repo(k) | Row::Dir { k, .. } | Row::File(k, ..) => Some(k),
            Row::Clean => None,
        }
    }

    /// How many folders in it is; a repository is none.
    pub(crate) fn depth(self) -> usize {
        match self {
            Row::Repo(_) | Row::Clean => 0,
            Row::Dir { depth, .. } | Row::File(.., depth) => depth,
        }
    }
}

/// Parts `from..to` of `path`, split at `/`.
pub(crate) fn path_parts(path: &str, from: usize, to: usize) -> &str {
    // Where part `n` starts.
    let cut = |n: usize| match n {
        0 => Some(0),
        n => path.match_indices('/').nth(n - 1).map(|(i, _)| i + 1),
    };
    let start = cut(from).unwrap_or(path.len());
    let end = cut(to).map_or(path.len(), |i| i - 1);
    &path[start..end.max(start)]
}

/// A folder of changed files: the folders in it, its files by name with
/// their index into the changes, and the same for the folders git lists
/// whole, as one change (untracked ones, `new/`).
#[derive(Default)]
pub(crate) struct Folder<'a> {
    folders: BTreeMap<&'a str, Folder<'a>>,
    files: Vec<(&'a str, usize)>,
    whole: Vec<(&'a str, usize)>,
}

/// Names in the order the tree lists them: letter case aside first.
pub(crate) fn by_name(a: &str, b: &str) -> Ordering {
    let lower = |s: &str| s.to_lowercase();
    lower(a).cmp(&lower(b)).then_with(|| a.cmp(b))
}

/// Repository `k`'s file lines, folded or not: as `file_order` has them;
/// or by folder, each folder's folders before its files, by name.
pub(crate) fn file_rows(k: usize, changes: &[Change], grouping: Grouping) -> Vec<Row> {
    if grouping == Grouping::List {
        let order = file_order(changes).into_iter();
        return order.map(|c| Row::File(k, c, 0)).collect();
    }
    let mut top = Folder::default();
    for (c, change) in changes.iter().enumerate() {
        let path = change.path.strip_suffix('/');
        let whole = path.is_some();
        let (dirs, name) = match path.unwrap_or(&change.path).rsplit_once('/') {
            Some((dirs, name)) => (Some(dirs), name),
            None => (None, path.unwrap_or(&change.path)),
        };
        let mut folder = &mut top;
        for part in dirs.into_iter().flat_map(|d| d.split('/')) {
            folder = folder.folders.entry(part).or_default();
        }
        match whole {
            true => folder.whole.push((name, c)),
            false => folder.files.push((name, c)),
        }
    }
    let mut rows = Vec::new();
    folder_rows(&top, k, (0, 0), &mut rows);
    rows
}

/// The order of `changes` in the tree, by index, folded or not.
pub(crate) fn tree_order(changes: &[Change], grouping: Grouping) -> Vec<usize> {
    let rows = file_rows(0, changes, grouping).into_iter();
    rows.filter_map(|r| match r {
        Row::File(_, c, _) => Some(c),
        _ => None,
    })
    .collect()
}

/// The lines of `folder` of repository `k`, `at` parts down its paths
/// and `depth` folders in, onto `rows`.
pub(crate) fn folder_rows(
    folder: &Folder,
    k: usize,
    (at, depth): (usize, usize),
    rows: &mut Vec<Row>,
) {
    // The folders git lists whole go by name among the others, a line
    // each.
    let mut folders: Vec<(&str, Option<&Folder>, usize)> = folder
        .folders
        .iter()
        .map(|(name, sub)| (*name, Some(sub), 0))
        .chain(folder.whole.iter().map(|&(name, c)| (name, None, c)))
        .collect();
    folders.sort_by(|a, b| by_name(a.0, b.0));
    for (_, sub, c) in folders {
        let Some(mut sub) = sub else {
            rows.push(Row::File(k, c, depth));
            continue;
        };
        let mut to = at + 1;
        while sub.files.is_empty()
            && sub.whole.is_empty()
            && sub.folders.len() == 1
            && let Some(only) = sub.folders.values().next()
        {
            sub = only;
            to += 1;
        }
        let line = rows.len();
        rows.push(Row::Dir {
            k,
            c: 0,
            from: at,
            to,
            depth,
        });
        folder_rows(sub, k, (to, depth + 1), rows);
        let first = rows[line..].iter().find_map(|r| match r {
            Row::File(_, c, _) => Some(*c),
            _ => None,
        });
        if let (Row::Dir { c, .. }, Some(first)) = (&mut rows[line], first) {
            *c = first;
        }
    }
    let mut files = folder.files.clone();
    files.sort_by(|a, b| by_name(a.0, b.0));
    rows.extend(files.into_iter().map(|(_, c)| Row::File(k, c, depth)));
}

/// The order `changes` are listed in, by index: conflicted ones first,
/// then the rest as git lists them.
pub(crate) fn file_order(changes: &[Change]) -> Vec<usize> {
    let (mut order, mut rest): (Vec<usize>, Vec<usize>) =
        (0..changes.len()).partition(|&c| changes[c].conflicted());
    order.append(&mut rest);
    order
}

/// How a change is shown: its path, a rename as `old → new`.
pub(crate) fn change_text(change: &Change) -> String {
    match &change.orig {
        Some(orig) => format!("{orig} → {}", change.path),
        None => change.path.clone(),
    }
}

/// How a change is shown under its folder: its name, a rename as
/// `old → new`, the old one by its name too when in the same folder.
pub(crate) fn file_name_text(change: &Change) -> String {
    let split = |path: &str| -> (String, String) {
        match path.rsplit_once('/') {
            Some((dir, name)) => (dir.to_owned(), name.to_owned()),
            None => (String::new(), path.to_owned()),
        }
    };
    // A folder git lists whole keeps its `/`.
    let (dir, mut name) = split(change.path.trim_end_matches('/'));
    if change.path.ends_with('/') {
        name.push('/');
    }
    match &change.orig {
        Some(orig) if split(orig).0 == dir => format!("{} → {name}", split(orig).1),
        Some(orig) => format!("{orig} → {name}"),
        None => name,
    }
}

/// The folded folders of a tree, by repository and path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folds(HashSet<(PathBuf, String)>);

impl Folds {
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Whether folder `path` of the repository at `root` shows its files.
    pub fn open(&self, root: &Path, path: &str) -> bool {
        // Asked per line per frame: no copies while nothing is folded.
        self.0.is_empty() || !self.0.contains(&(root.to_path_buf(), path.to_owned()))
    }

    /// Show or hide the files of folder `path` of the repository at `root`.
    pub fn set_open(&mut self, root: &Path, path: &str, open: bool) {
        let key = (root.to_path_buf(), path.to_owned());
        if open {
            self.0.remove(&key);
        } else {
            self.0.insert(key);
        }
    }

    /// Unfold the folders the file at `path` of the repository at `root`
    /// is in.
    pub fn reveal(&mut self, root: &Path, path: &str) {
        self.0.retain(|(r, dir)| !(r == root && in_dir(path, dir)));
    }
}

/// The path of folder line `row` of a repository with `changes`.
pub(crate) fn dir_path(row: Row, changes: &[Change]) -> Option<&str> {
    let Row::Dir { c, to, .. } = row else {
        return None;
    };
    Some(path_parts(&changes[c].path, 0, to))
}

/// What a key or a click on a `FileTree` asks of its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// The selected line changed: the diff goes to it.
    Show,
    /// The keyboard goes to the diff, on the selected line.
    ToDiff,
    /// Tick the line's files, or untick them: of the repository at `root`.
    Tick { root: PathBuf, paths: Vec<String> },
}

/// A list of the changed files beside a diff that has no panel of its own
/// around it (a feature's code review): its lines, as the repositories
/// panel's tree has them, and what the keyboard and the pointer do with them.
#[derive(Debug, Default)]
pub struct FileTree {
    pub grouping: Grouping,
    /// The selected line, by who it is.
    pub selected: Option<Selected>,
    /// Where it last stood, for when it goes.
    selected_at: usize,
    /// The repositories folded, and those with too many files to show
    /// unasked that are not.
    folded: HashSet<PathBuf>,
    unfolded: HashSet<PathBuf>,
    pub folds: Folds,
    pub scroll: Scroll,
    /// It has the keyboard, not the diff.
    pub focused: bool,
}

/// The checkbox over files with these checkboxes: on when all are, partly
/// when some are or any changed since, else off.
pub fn check_over(files: impl IntoIterator<Item = Check>) -> Check {
    let (mut any, mut all, mut some) = (false, true, false);
    for check in files {
        any = true;
        all &= check == Check::On;
        some |= check != Check::Off;
    }
    match (any && all, some) {
        (true, _) => Check::On,
        (false, true) => Check::Partial,
        _ => Check::Off,
    }
}

impl FileTree {
    pub fn new(grouping: Grouping) -> Self {
        FileTree {
            grouping,
            ..FileTree::default()
        }
    }

    /// Whether `section`'s files show.
    fn open(&self, section: &Section) -> bool {
        let n = section.changes.len();
        n > 0
            && (self.unfolded.contains(section.root)
                || (!self.folded.contains(section.root) && n < crate::repos::FOLD_FILES))
    }

    /// Show or hide `section`'s files.
    fn set_open(&mut self, section: &Section, open: bool) {
        if section.changes.is_empty() {
            return;
        }
        let root = section.root.to_path_buf();
        if open {
            self.folded.remove(&root);
            self.unfolded.insert(root);
        } else {
            self.unfolded.remove(&root);
            self.folded.insert(root);
        }
    }

    /// The tree's lines: each repository with changes, then its files when
    /// unfolded, as `file_rows` has them, but those in folded folders.
    fn rows(&self, sections: &[Section]) -> Vec<Row> {
        let mut rows = Vec::new();
        for (k, section) in sections.iter().enumerate() {
            if section.changes.is_empty() {
                continue;
            }
            rows.push(Row::Repo(k));
            if !self.open(section) {
                continue;
            }
            // The depth of the folded folder whose lines are skipped.
            let mut hiding = None;
            for row in file_rows(k, section.changes, self.grouping) {
                if hiding.is_some_and(|d| row.depth() > d) {
                    continue;
                }
                hiding = None;
                if !self.dir_open(row, section) {
                    hiding = Some(row.depth());
                }
                rows.push(row);
            }
        }
        rows
    }

    /// Whether `row`, a folder's line, shows its files; true of any other.
    fn dir_open(&self, row: Row, section: &Section) -> bool {
        dir_path(row, section.changes).is_none_or(|path| self.folds.open(section.root, path))
    }

    /// Who line `row` is.
    fn who(row: Row, sections: &[Section]) -> Option<Selected> {
        let section = sections.get(row.item()?)?;
        let root = section.root.to_path_buf();
        Some(match row {
            Row::Repo(_) => Selected::Repo(root),
            Row::Dir { .. } => Selected::Dir(root, dir_path(row, section.changes)?.to_owned()),
            Row::File(_, c, _) => Selected::File(root, section.changes.get(c)?.path.clone()),
            Row::Clean => return None,
        })
    }

    /// The selected line, by index into `rows`.
    fn selected_row(&self, rows: &[Row], sections: &[Section]) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        (rows.iter()).position(|&row| Self::who(row, sections).as_ref() == Some(selected))
    }

    /// Where the diff goes for the selected line.
    pub fn place(&self) -> Option<Place<'_>> {
        Some(match self.selected.as_ref()? {
            Selected::Repo(root) => Place::Section(root),
            Selected::Dir(root, dir) => Place::Dir(root, dir),
            Selected::File(root, path) => Place::File(root, path),
            Selected::Clean => return None,
        })
    }

    /// The selected file.
    pub fn selected_file(&self) -> Option<(&Path, &str)> {
        match self.selected.as_ref()? {
            Selected::File(root, path) => Some((root, path)),
            _ => None,
        }
    }

    /// Select line `k` of `rows`, in view on a page of `page`: whether
    /// another line is selected now.
    fn select(&mut self, rows: &[Row], k: usize, sections: &[Section], page: usize) -> bool {
        let Some(id) = rows.get(k).and_then(|&row| Self::who(row, sections)) else {
            return false;
        };
        let moved = self.selected.as_ref() != Some(&id);
        self.selected = Some(id);
        self.selected_at = k;
        let to = showing(self.scroll.pos(), (k, k), page.max(1));
        move_to(&mut self.scroll, to);
        moved
    }

    /// The lines changed (a fold, the diff read again): the selection kept
    /// on who it was, else near where it was, and the view within them.
    pub fn settle(&mut self, sections: &[Section], page: usize) {
        let rows = self.rows(sections);
        match self.selected_row(&rows, sections) {
            Some(k) => self.selected_at = k,
            None if !rows.is_empty() => {
                let k = self.selected_at.min(rows.len() - 1);
                self.select(&rows, k, sections, page);
            }
            None => self.selected = None,
        }
        let last = rows.len().saturating_sub(page.max(1)) as f64;
        if self.scroll.pos() > last {
            self.scroll.jump(last);
        }
    }

    /// The diff's cursor went to `moved`: its line is selected, and shows.
    pub fn follow(&mut self, moved: Moved, sections: &[Section], page: usize) {
        let id = match moved {
            Moved::Section(root) => Selected::Repo(root),
            Moved::File(root, path) => {
                if let Some(section) = sections.iter().find(|s| s.root == root) {
                    self.set_open(section, true);
                }
                self.folds.reveal(&root, &path);
                Selected::File(root, path)
            }
        };
        let rows = self.rows(sections);
        let at = (rows.iter()).position(|&row| Self::who(row, sections).as_ref() == Some(&id));
        if let Some(k) = at {
            self.select(&rows, k, sections, page);
        }
    }

    /// Unfold every repository and folder, or fold them all (the folders
    /// while listed by folder): the selection, hidden, on its repository's
    /// line.
    pub fn fold_all(&mut self, open: bool, sections: &[Section], page: usize) {
        for section in sections {
            self.set_open(section, open);
        }
        if open {
            self.folds.clear();
        } else {
            if self.grouping == Grouping::Folders {
                for (k, section) in sections.iter().enumerate() {
                    for row in file_rows(k, section.changes, self.grouping) {
                        if let Some(path) = dir_path(row, section.changes) {
                            self.folds.set_open(section.root, path, false);
                        }
                    }
                }
            }
            if let Some(Selected::Dir(root, _) | Selected::File(root, _)) = &self.selected {
                self.selected = Some(Selected::Repo(root.clone()));
            }
        }
        self.relist(sections, page);
    }

    /// List the files as `grouping` says, the selection kept (a folder's
    /// line, gone from a list, gives way to its repository's) and in view.
    /// The diff's host puts the diff in the tree's new order.
    pub fn set_grouping(&mut self, grouping: Grouping, sections: &[Section], page: usize) {
        self.grouping = grouping;
        if let Some(Selected::Dir(root, _)) = &self.selected {
            self.selected = Some(Selected::Repo(root.clone()));
        }
        self.relist(sections, page);
    }

    /// The lines changed as a whole: the selection kept, and in view.
    fn relist(&mut self, sections: &[Section], page: usize) {
        self.settle(sections, page);
        let rows = self.rows(sections);
        if let Some(k) = self.selected_row(&rows, sections) {
            let to = showing(self.scroll.pos(), (k, k), page.max(1));
            move_to(&mut self.scroll, to);
        }
    }

    /// Where each file of a repository with `changes` goes in the tree's
    /// order, by path.
    pub fn rank<'a>(&self, changes: &'a [Change]) -> HashMap<&'a str, usize> {
        (tree_order(changes, self.grouping).into_iter())
            .enumerate()
            .map(|(n, c)| (changes[c].path.as_str(), n))
            .collect()
    }

    /// The files line `row` stands for: its repository's top folder and
    /// their paths.
    fn files_of(row: Row, sections: &[Section]) -> Option<(PathBuf, Vec<String>)> {
        let section = sections.get(row.item()?)?;
        let paths = match row {
            Row::Repo(_) => section.changes.iter().map(|c| c.path.clone()).collect(),
            Row::Dir { .. } => {
                let dir = dir_path(row, section.changes)?;
                (section.changes.iter())
                    .filter(|c| in_dir(&c.path, dir))
                    .map(|c| c.path.clone())
                    .collect()
            }
            Row::File(_, c, _) => vec![section.changes.get(c)?.path.clone()],
            Row::Clean => return None,
        };
        Some((section.root.to_path_buf(), paths))
    }

    /// Enter, or a press on the expander, on line `row`: a repository or a
    /// folder folds or unfolds.
    fn fold_line(&mut self, row: Row, sections: &[Section], page: usize) {
        let Some(section) = row.item().and_then(|k| sections.get(k)) else {
            return;
        };
        match row {
            Row::Repo(_) => {
                let open = !self.open(section);
                self.set_open(section, open);
            }
            Row::Dir { .. } => {
                let open = !self.dir_open(row, section);
                if let Some(path) = dir_path(row, section.changes) {
                    self.folds.set_open(section.root, path, open);
                }
            }
            _ => return,
        }
        self.settle(sections, page);
    }

    /// A key while it has the keyboard: whether it took it, and what that
    /// asks of the host.
    pub fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        sections: &[Section],
        page: usize,
    ) -> Option<Option<Asked>> {
        let page = page.max(1);
        let rows = self.rows(sections);
        let at = self.selected_row(&rows, sections);
        let n = rows.len() as isize;
        let step = |by: isize| match at {
            _ if n == 0 => None,
            Some(k) => Some((k as isize + by).clamp(0, n - 1) as usize),
            None => Some(0),
        };
        let plain = !(mods.ctrl || mods.alt || mods.logo || mods.shift);
        let only_ctrl = mods.ctrl && !(mods.shift || mods.alt || mods.logo);
        let moved = match sym {
            Keysym::Down | Keysym::KP_Down if plain => step(1),
            Keysym::Up | Keysym::KP_Up if plain => step(-1),
            Keysym::j | Keysym::J if only_ctrl => step(1),
            Keysym::k | Keysym::K if only_ctrl => step(-1),
            Keysym::Page_Down | Keysym::KP_Page_Down if plain => step(page as isize),
            Keysym::Page_Up | Keysym::KP_Page_Up if plain => step(-(page as isize)),
            Keysym::Home | Keysym::KP_Home if plain => step(-n),
            Keysym::End | Keysym::KP_End if plain => step(n),
            _ => None,
        };
        if let Some(k) = moved {
            let shown = self.select(&rows, k, sections, page);
            return Some(shown.then_some(Asked::Show));
        }
        if !plain {
            return None;
        }
        let row = at.map(|k| rows[k]);
        let section = |row: Row| row.item().and_then(|k| sections.get(k));
        match sym {
            Keysym::space => Some(row.and_then(|row| {
                let (root, paths) = Self::files_of(row, sections)?;
                Some(Asked::Tick { root, paths })
            })),
            Keysym::Right | Keysym::KP_Right => {
                match row {
                    Some(Row::Repo(k)) => self.set_open(&sections[k], true),
                    Some(row @ Row::Dir { .. })
                        if section(row).is_some_and(|s| !self.dir_open(row, s)) =>
                    {
                        self.fold_line(row, sections, page);
                    }
                    _ => {}
                }
                Some(None)
            }
            // A line folds, else the one over it is selected.
            Keysym::Left | Keysym::KP_Left => {
                let fold = |row: Row| match row {
                    Row::Repo(k) => self.open(&sections[k]),
                    Row::Dir { .. } => section(row).is_some_and(|s| self.dir_open(row, s)),
                    _ => false,
                };
                match (row, at) {
                    (Some(row), _) if fold(row) => self.fold_line(row, sections, page),
                    (Some(_), Some(at)) => {
                        let depth = rows[at].depth();
                        let up = rows[..at].iter().rposition(|r| match r {
                            Row::Repo(_) | Row::Clean => true,
                            Row::Dir { depth: d, .. } => *d < depth,
                            Row::File(..) => false,
                        });
                        if let Some(up) = up {
                            let shown = self.select(&rows, up, sections, page);
                            return Some(shown.then_some(Asked::Show));
                        }
                    }
                    _ => {}
                }
                Some(None)
            }
            Keysym::Return | Keysym::KP_Enter => match row {
                Some(Row::File(..)) => Some(Some(Asked::ToDiff)),
                Some(row) => {
                    self.fold_line(row, sections, page);
                    Some(None)
                }
                None => Some(None),
            },
            _ => None,
        }
    }

    /// Line `i` of those in view, by index into `rows`.
    fn at(&self, i: usize, rows: &[Row], page: usize) -> Option<usize> {
        let (first, _) = standing(&self.scroll, rows.len(), page);
        (first + i < rows.len()).then_some(first + i)
    }

    /// A press on line `i` of those in view, which takes the keyboard.
    pub fn press_line(&mut self, i: usize, sections: &[Section], page: usize) -> Option<Asked> {
        let rows = self.rows(sections);
        let k = self.at(i, &rows, page)?;
        self.focused = true;
        self.select(&rows, k, sections, page).then_some(Asked::Show)
    }

    /// A press on the checkbox of line `i` of those in view.
    pub fn press_check(&mut self, i: usize, sections: &[Section], page: usize) -> Option<Asked> {
        let rows = self.rows(sections);
        let k = self.at(i, &rows, page)?;
        self.focused = true;
        self.select(&rows, k, sections, page);
        let (root, paths) = Self::files_of(rows[k], sections)?;
        Some(Asked::Tick { root, paths })
    }

    /// A press on the expander of line `i` of those in view.
    pub fn press_expander(&mut self, i: usize, sections: &[Section], page: usize) {
        let rows = self.rows(sections);
        let Some(k) = self.at(i, &rows, page) else {
            return;
        };
        self.focused = true;
        self.select(&rows, k, sections, page);
        self.fold_line(rows[k], sections, page);
    }

    /// The file on line `i` of those in view, if a file is there.
    pub fn file_at(
        &self,
        i: usize,
        sections: &[Section],
        page: usize,
    ) -> Option<(PathBuf, String)> {
        let rows = self.rows(sections);
        let row = rows[self.at(i, &rows, page)?];
        matches!(row, Row::File(..))
            .then(|| Self::files_of(row, sections))
            .flatten()
            .and_then(|(root, mut paths)| Some((root, paths.pop()?)))
    }

    /// How far the lines can scroll, on a page of `page`.
    pub fn bounds(&self, sections: &[Section], page: usize, density: Density) -> Bounds {
        bounds(
            self.rows(sections).len(),
            page,
            crate::chrome::repos_row(density),
        )
    }

    /// Line `row` as the tree draws it, `check` saying each file's checkbox.
    fn line(&self, row: Row, sections: &[Section], diff: &DiffView, check: CheckOf) -> TreeLine {
        let section = &sections[row.item().unwrap_or_default()];
        let files = (diff.diffs.get(section.root)).map_or(&[][..], |(_, files)| files.as_slice());
        let over = |paths: &mut dyn Iterator<Item = &Change>| {
            check_over(paths.map(|c| check(section.root, &c.path)))
        };
        match row {
            Row::Dir {
                from, to, depth, ..
            } => {
                let path = dir_path(row, section.changes).unwrap_or_default();
                TreeLine::Dir(DirLine {
                    check: over(&mut section.changes.iter().filter(|c| in_dir(&c.path, path))),
                    open: self.dir_open(row, section),
                    name: path_parts(path, from, to).to_owned(),
                    depth,
                })
            }
            Row::File(_, c, depth) => {
                let change = &section.changes[c];
                let diff_of = files.iter().find(|f| f.path == change.path);
                TreeLine::File(FileLine {
                    check: check(section.root, &change.path),
                    code: change.code,
                    path: match self.grouping {
                        Grouping::List => change_text(change),
                        Grouping::Folders => file_name_text(change),
                    },
                    conflicted: change.conflicted(),
                    depth,
                    added: diff_of.map_or(0, |f| f.added),
                    removed: diff_of.map_or(0, |f| f.removed),
                    edited: false,
                    comments: (diff.review.comments.iter())
                        .filter(|c| c.root == section.root && c.path == change.path)
                        .count(),
                })
            }
            _ => TreeLine::Repo(RepoLine {
                check: over(&mut section.changes.iter()),
                expanded: (!section.changes.is_empty()).then(|| self.open(section)),
                label: section.label.to_owned(),
                branch: section.branch.map(str::to_owned),
                dirty: false,
                ahead: 0,
                behind: 0,
                no_upstream: false,
                stashes: 0,
                files: section.changes.len(),
                added: files.iter().map(|f| f.added).sum(),
                removed: files.iter().map(|f| f.removed).sum(),
                chip: None,
                state: LineState::Settled,
            }),
        }
    }

    /// Put its lines in view on `view`, on a page of `page`.
    pub fn fill(
        &self,
        view: &mut ReposView,
        sections: &[Section],
        diff: &DiffView,
        check: CheckOf,
        page: usize,
    ) {
        let page = page.max(1);
        let rows = self.rows(sections);
        let (first, part) = standing(&self.scroll, rows.len(), page);
        // Scrolled part of a line, the tree shows one more under it.
        let end = (first + page + usize::from(part > 0.0)).min(rows.len());
        let shown = &rows[first..end];
        view.lines = (shown.iter())
            .map(|&row| self.line(row, sections, diff, check))
            .collect();
        view.part = part;
        view.over = self.scroll.overscroll();
        view.first = first;
        view.total = rows.len();
        view.selected = self
            .selected_row(&rows, sections)
            .filter(|k| (first..first + shown.len()).contains(k))
            .map(|k| k - first);
        view.grouping = self.grouping;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str) -> Change {
        Change {
            path: path.into(),
            orig: None,
            code: 'M',
        }
    }

    fn section<'a>(root: &'a Path, changes: &'a [Change]) -> Section<'a> {
        Section {
            root,
            label: "api",
            branch: Some("feat"),
            head: None,
            changes,
        }
    }

    fn no_mods() -> ModifiersState {
        ModifiersState::default()
    }

    #[test]
    fn it_lists_flat_or_by_folder_and_folds_either() {
        let root = PathBuf::from("/r");
        let changes = [change("src/a.rs"), change("src/b.rs"), change("top.rs")];
        let sections = [section(&root, &changes)];
        let mut tree = FileTree::new(Grouping::List);
        assert_eq!(tree.rows(&sections).len(), 4);
        tree.set_grouping(Grouping::Folders, &sections, 10);
        let rows = tree.rows(&sections);
        assert_eq!(rows.len(), 5);
        assert!(matches!(rows[1], Row::Dir { .. }));
        // The folder folds, then the repository.
        tree.selected = Some(Selected::Dir(root.clone(), "src".into()));
        tree.fold_line(rows[1], &sections, 10);
        assert_eq!(tree.rows(&sections).len(), 3);
        tree.fold_all(false, &sections, 10);
        assert_eq!(tree.rows(&sections).len(), 1);
        assert_eq!(tree.selected, Some(Selected::Repo(root.clone())));
        tree.fold_all(true, &sections, 10);
        assert_eq!(tree.rows(&sections).len(), 5);
    }

    #[test]
    fn the_diff_moving_selects_and_unfolds_its_line() {
        let root = PathBuf::from("/r");
        let changes = [change("src/a.rs"), change("top.rs")];
        let sections = [section(&root, &changes)];
        let mut tree = FileTree::new(Grouping::Folders);
        tree.fold_all(false, &sections, 10);
        tree.follow(Moved::File(root.clone(), "src/a.rs".into()), &sections, 10);
        assert_eq!(
            tree.selected,
            Some(Selected::File(root.clone(), "src/a.rs".into()))
        );
        let rows = tree.rows(&sections);
        assert!(tree.selected_row(&rows, &sections).is_some());
    }

    #[test]
    fn space_asks_to_tick_the_lines_files() {
        let root = PathBuf::from("/r");
        let changes = [change("src/a.rs"), change("src/b.rs"), change("top.rs")];
        let sections = [section(&root, &changes)];
        let mut tree = FileTree::new(Grouping::Folders);
        let paths = |tree: &mut FileTree| match tree.key(&no_mods(), Keysym::space, &sections, 10) {
            Some(Some(Asked::Tick { paths, .. })) => paths,
            other => panic!("{other:?}"),
        };
        tree.selected = Some(Selected::Repo(root.clone()));
        assert_eq!(paths(&mut tree).len(), 3);
        tree.selected = Some(Selected::Dir(root.clone(), "src".into()));
        assert_eq!(paths(&mut tree), ["src/a.rs", "src/b.rs"]);
        tree.selected = Some(Selected::File(root.clone(), "top.rs".into()));
        assert_eq!(paths(&mut tree), ["top.rs"]);
    }

    #[test]
    fn keys_move_fold_and_go_to_the_diff() {
        let root = PathBuf::from("/r");
        let changes = [change("src/a.rs"), change("top.rs")];
        let sections = [section(&root, &changes)];
        let mut tree = FileTree::new(Grouping::Folders);
        let mods = no_mods();
        // Down from nothing selects the first line.
        assert_eq!(
            tree.key(&mods, Keysym::Down, &sections, 10),
            Some(Some(Asked::Show))
        );
        assert_eq!(tree.selected, Some(Selected::Repo(root.clone())));
        tree.key(&mods, Keysym::Down, &sections, 10);
        assert_eq!(
            tree.selected,
            Some(Selected::Dir(root.clone(), "src".into()))
        );
        // Left folds it; Left again goes up; Right on the folder unfolds.
        tree.key(&mods, Keysym::Left, &sections, 10);
        assert_eq!(tree.rows(&sections).len(), 3);
        tree.key(&mods, Keysym::Right, &sections, 10);
        assert_eq!(tree.rows(&sections).len(), 4);
        tree.key(&mods, Keysym::Down, &sections, 10);
        assert_eq!(
            tree.key(&mods, Keysym::Return, &sections, 10),
            Some(Some(Asked::ToDiff))
        );
        // The file's line goes up to its folder.
        assert_eq!(
            tree.key(&mods, Keysym::Left, &sections, 10),
            Some(Some(Asked::Show))
        );
        assert_eq!(tree.selected, Some(Selected::Dir(root, "src".into())));
    }

    #[test]
    fn a_folder_or_repository_checkbox_says_what_its_files_say() {
        use Check::*;
        assert_eq!(check_over([On, On]), On);
        assert_eq!(check_over([On, Off]), Partial);
        assert_eq!(check_over([Partial, Off]), Partial);
        assert_eq!(check_over([Off, Off]), Off);
        assert_eq!(check_over([]), Off);
    }

    #[test]
    fn folds_are_kept_by_repository_and_reveal_opens_a_files_folders() {
        let (a, b) = (PathBuf::from("/a"), PathBuf::from("/b"));
        let mut folds = Folds::default();
        folds.set_open(&a, "src", false);
        folds.set_open(&a, "src/x", false);
        folds.set_open(&b, "src", false);
        assert!(!folds.open(&a, "src"));
        folds.reveal(&a, "src/x/f.rs");
        assert!(folds.open(&a, "src") && folds.open(&a, "src/x"));
        assert!(!folds.open(&b, "src"));
    }
}
