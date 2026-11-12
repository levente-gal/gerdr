//! The new feature dialog: its rows, keys and what it makes.

use super::*;

impl NewFeatureDialog {
    pub(super) fn new(project: String, serial: u64, from: PathBuf) -> Self {
        Self {
            project,
            serial,
            title: TextField::default(),
            root: Finding::Looking,
            touched: Vec::new(),
            focus: None,
            top: 0,
            error: None,
            hover: None,
            from,
            others: Vec::new(),
            picking: false,
            bases: BTreeMap::new(),
            choosing: None,
            query: TextField::default(),
            review: None,
            link_first: Vec::new(),
        }
    }

    /// The root was found, or not: every repository of the project is
    /// offered. Of several, the one it was opened from is ticked.
    pub(super) fn found(&mut self, result: Result<Root, String>) {
        match result {
            Ok(root) => {
                self.touched = root
                    .repos
                    .iter()
                    .map(|r| !root.multi || self.from.starts_with(&r.main))
                    .collect();
                self.root = Finding::Found(root);
            }
            Err(why) => self.root = Finding::Failed(why),
        }
        self.error = None;
    }

    /// Project `project`'s root was found, or not. Nothing of it is ticked,
    /// unless it has only one repository to link it for.
    pub(super) fn found_other(&mut self, project: &str, result: Result<Root, String>) {
        let Some(k) = self.others.iter().position(|o| o.project == project) else {
            return;
        };
        let other = &mut self.others[k];
        other.root = match result {
            Ok(root) if root.repos.is_empty() => Finding::Failed("no repositories in it".into()),
            Ok(root) => {
                other.touched = vec![root.repos.len() == 1; root.repos.len()];
                Finding::Found(root)
            }
            Err(why) => Finding::Failed(why),
        };
    }

    /// Its rows: the branches while choosing a base, the projects while
    /// picking; else the repositories, the linked projects' after the home
    /// root's.
    pub(super) fn rows(&self) -> Vec<DialogRow> {
        if self.choosing.is_some() {
            let n = match self.review {
                Some(_) => self.pull_rows().len(),
                None => self.branch_rows().len(),
            };
            return (0..n).map(DialogRow::Branch).collect();
        }
        if self.picking {
            return (0..self.others.len()).map(DialogRow::Project).collect();
        }
        let mut rows: Vec<DialogRow> = match &self.root {
            Finding::Found(root) => (0..root.repos.len()).map(DialogRow::Home).collect(),
            _ => Vec::new(),
        };
        for (k, other) in self.others.iter().enumerate() {
            if let (true, Finding::Found(root)) = (other.linked, &other.root) {
                rows.extend((0..root.repos.len()).map(|i| DialogRow::Linked(k, i)));
            }
        }
        rows
    }

    /// Whether its rows can be ticked or picked: branches, projects, linked
    /// repositories, and the home root's when it holds several.
    pub(super) fn multi(&self) -> bool {
        self.picking
            || self.choosing.is_some()
            || self.review.is_some()
            || matches!(&self.root, Finding::Found(root) if root.multi)
            || self.others.iter().any(|o| o.linked)
    }

    pub(super) fn repos(&self) -> usize {
        self.rows().len()
    }

    /// The row slots it makes room for: a filter narrowing the branches
    /// doesn't resize it, or the field being typed in would move.
    pub(super) fn slots(&self) -> usize {
        // A picker over the form is no shorter than the form: the dialog
        // doesn't jump as it opens and closes.
        match (&self.choosing, &self.review) {
            (Some(_), Some(_)) => self.pulls_offered().len().max(self.form_rows()),
            (Some(_), None) => self.branches_offered().len().max(self.form_rows()),
            (None, _) => self.repos(),
        }
    }

    /// How many rows the form has, its repositories, under the picker.
    fn form_rows(&self) -> usize {
        match &self.root {
            Finding::Found(root) => root.repos.len(),
            _ => 0,
        }
    }

    /// The repository `key` as found.
    pub(super) fn repo_found(&self, key: &RepoKey) -> Option<&Found> {
        let finding = match &key.0 {
            None => &self.root,
            Some(project) => &self.others.iter().find(|o| o.project == *project)?.root,
        };
        let Finding::Found(root) = finding else {
            return None;
        };
        root.repos.iter().find(|r| r.name == key.1)
    }

    /// `repo_found`, to change.
    fn repo_found_mut(&mut self, key: &RepoKey) -> Option<&mut Found> {
        let finding = match &key.0 {
            None => &mut self.root,
            Some(project) => &mut self.others.iter_mut().find(|o| o.project == *project)?.root,
        };
        let Finding::Found(root) = finding else {
            return None;
        };
        root.repos.iter_mut().find(|r| r.name == key.1)
    }

    /// The repository of row `row`, if it is one.
    pub(super) fn row_repo(&self, row: DialogRow) -> Option<RepoKey> {
        let (project, finding, i) = match row {
            DialogRow::Home(i) => (None, &self.root, i),
            DialogRow::Linked(k, i) => {
                let other = self.others.get(k)?;
                (Some(other.project.clone()), &other.root, i)
            }
            DialogRow::Project(_) | DialogRow::Branch(_) => return None,
        };
        let Finding::Found(root) = finding else {
            return None;
        };
        Some((project, root.repos.get(i)?.name.clone()))
    }

    /// The repositories in it whose branches nobody looked for yet: the home
    /// root's, and those of the projects linked. Noted as looked for.
    pub(super) fn unlooked(&mut self) -> Vec<(RepoKey, Found)> {
        let root_of = |project: Option<&String>, finding: &Finding| match finding {
            Finding::Found(root) => (root.repos.iter())
                .map(|r| ((project.cloned(), r.name.clone()), r.clone()))
                .collect(),
            _ => Vec::new(),
        };
        let mut all: Vec<(RepoKey, Found)> = root_of(None, &self.root);
        for other in self.others.iter().filter(|o| o.linked) {
            all.extend(root_of(Some(&other.project), &other.root));
        }
        all.retain(|(key, _)| !self.bases.contains_key(key));
        for (key, found) in &all {
            self.bases.insert(
                key.clone(),
                RepoBranches {
                    default: found.base.clone(),
                    ..Default::default()
                },
            );
        }
        all
    }

    /// The branches of repository `key` were read, or not.
    pub(super) fn base_branches(&mut self, key: &RepoKey, result: Result<Vec<String>, String>) {
        if let Some(entry) = self.bases.get_mut(key) {
            entry.branches = Some(result);
        }
    }

    /// Origin says `branch` is the default of repository `key`: its features
    /// start from it, unless the user picked or it was never fetched, which
    /// `create` couldn't start from. Then what git has here stands, and the
    /// log says why.
    pub(super) fn remote_base(&mut self, key: &RepoKey, branch: String) {
        let Some(entry) = self.bases.get_mut(key) else {
            return;
        };
        let known = matches!(&entry.branches, Some(Ok(all)) if all.contains(&branch));
        if !known {
            tracing::warn!("{}: origin's default {branch} isn't fetched", key.1);
            return;
        }
        if entry.picked || entry.default == branch {
            return;
        }
        entry.default = branch.clone();
        if let Some(found) = self.repo_found_mut(key) {
            found.base = branch;
        }
    }

    /// The branches of the repository being chosen for: its default first,
    /// then the one picked, then the newest.
    fn branches_offered(&self) -> Vec<String> {
        let Some(choosing) = &self.choosing else {
            return Vec::new();
        };
        let entry = self.bases.get(&choosing.repo);
        let current = self.repo_found(&choosing.repo).map(|r| r.base.clone());
        let listed = (entry.and_then(|e| e.branches.as_ref()))
            .and_then(|b| b.as_ref().ok())
            .map_or(&[][..], Vec::as_slice);
        let mut all: Vec<String> = (entry.map(|e| e.default.clone()).into_iter())
            .chain(current)
            .chain(listed.iter().cloned())
            .collect();
        let mut seen = BTreeSet::new();
        all.retain(|b| seen.insert(b.clone()));
        all
    }

    /// `branches_offered` the filter lets through.
    pub(super) fn branch_rows(&self) -> Vec<String> {
        let query = self.query.trim().to_lowercase();
        let mut all = self.branches_offered();
        all.retain(|b| b.to_lowercase().contains(&query));
        all
    }

    /// The open pull requests of the repository being chosen for.
    fn pulls_offered(&self) -> &[feature::pr::OpenPull] {
        let of = |c: &Choosing| self.review.as_ref()?.pulls.get(&c.repo);
        self.choosing
            .as_ref()
            .and_then(of)
            .map_or(&[], RepoPulls::listed)
    }

    /// `pulls_offered` the filter lets through: by number, title, branch or
    /// author.
    pub(super) fn pull_rows(&self) -> Vec<&feature::pr::OpenPull> {
        let query = self.query.trim().to_lowercase();
        (self.pulls_offered().iter())
            .filter(|p| {
                let text = format!("#{} {} {} {}", p.number, p.title, p.head_ref, p.author);
                text.to_lowercase().contains(&query)
            })
            .collect()
    }

    /// The repositories it shows whose pull requests nobody listed yet,
    /// with their main worktrees. Noted as listed.
    pub(super) fn unlisted(&mut self) -> Vec<(RepoKey, PathBuf)> {
        let rows = self.rows();
        let keys: Vec<RepoKey> = rows.into_iter().filter_map(|r| self.row_repo(r)).collect();
        let Some(review) = self.review.as_ref() else {
            return Vec::new();
        };
        let new: Vec<(RepoKey, PathBuf)> = (keys.into_iter())
            .filter(|key| !review.pulls.get(key).is_some_and(|e| e.asked))
            .filter_map(|key| {
                let main = self.repo_found(&key)?.main.clone();
                Some((key, main))
            })
            .collect();
        if let Some(review) = self.review.as_mut() {
            for (key, _) in &new {
                review.pulls.entry(key.clone()).or_default().asked = true;
            }
        }
        new
    }

    /// The open pull requests of repository `key` were listed, or not: the
    /// one it was opened for is picked, else one on a branch picked
    /// elsewhere.
    pub(super) fn pulls_listed(
        &mut self,
        key: &RepoKey,
        result: Result<Vec<feature::pr::OpenPull>, String>,
    ) {
        let Some(review) = self.review.as_mut() else {
            return;
        };
        let Some(entry) = review.pulls.get_mut(key) else {
            return;
        };
        entry.list = Some(result);
        let wanted = (review.wanted.as_ref()).and_then(|(host, slug, number)| {
            entry.listed().iter().find(|p| {
                crate::ci::pull_request_of(&p.url).is_some_and(|(h, s, n)| {
                    n == *number && h.eq_ignore_ascii_case(host) && s.eq_ignore_ascii_case(slug)
                })
            })
        });
        if let Some(number) = wanted.map(|p| p.number) {
            review.wanted = None;
            return self.pick_pull_of(key, number);
        }
        if let Some((_, slug, number)) = review
            .wanted
            .take_if(|_| (review.pulls.values()).all(|e| e.list.is_some()))
        {
            review.note = Some(format!(
                "#{number} of {slug} isn't in this project's repositories"
            ));
        }
        let Some(entry) = review.pulls.get(key) else {
            return;
        };
        // Changing a review, a repository it doesn't hold stays out unless
        // picked: it may have just been left out.
        if entry.chosen || entry.picked.is_some() || review.into.is_some() {
            return;
        }
        let heads: Vec<String> = (review.pulls.values())
            .filter_map(|e| e.picked_pull().map(|p| p.head_ref.clone()))
            .collect();
        let Some(entry) = review.pulls.get_mut(key) else {
            return;
        };
        let same =
            (heads.iter()).find_map(|head| Some((same_branch(entry.listed(), head)?.number, head)));
        if let Some((number, head)) = same {
            entry.picked = Some(number);
            review.note = Some(format!(
                "#{number} of {} picked too: same branch {head}",
                key.1
            ));
        }
    }

    /// Pick pull request `number` of repository `key`, and in each
    /// repository nobody chose for, its one open pull request on the same
    /// branch.
    pub(super) fn pick_pull_of(&mut self, key: &RepoKey, number: u64) {
        let Some(review) = self.review.as_mut() else {
            return;
        };
        let Some(entry) = review.pulls.get_mut(key) else {
            return;
        };
        entry.picked = Some(number);
        entry.chosen = true;
        let Some(head) = entry.picked_pull().map(|p| p.head_ref.clone()) else {
            return;
        };
        let mut also = Vec::new();
        for (other, entry) in review.pulls.iter_mut().filter(|(k, _)| *k != key) {
            if entry.chosen || entry.picked.is_some() {
                continue;
            }
            if let Some(number) = same_branch(entry.listed(), &head).map(|p| p.number) {
                entry.picked = Some(number);
                also.push(format!("#{number} of {}", other.1));
            }
        }
        review.note = (!also.is_empty())
            .then(|| format!("{} picked too: same branch {head}", also.join(", ")));
    }

    /// Pick pull request row `i` for the repository being chosen for.
    pub(super) fn pick_pull(&mut self, i: usize) {
        let Some(number) = self.pull_rows().get(i).map(|p| p.number) else {
            return;
        };
        let Some(repo) = self.choosing.as_ref().map(|c| c.repo.clone()) else {
            return;
        };
        self.pick_pull_of(&repo, number);
        self.close_bases();
    }

    /// Leave repository row `i` out of the review, or pick its pull
    /// request when none is.
    fn toggle_pull(&mut self, i: usize) {
        let Some(key) = self.rows().get(i).and_then(|r| self.row_repo(*r)) else {
            return;
        };
        let entry = self.review.as_mut().and_then(|r| r.pulls.get_mut(&key));
        match entry {
            Some(entry) if entry.picked.is_some() => {
                entry.picked = None;
                entry.chosen = true;
                self.error = None;
            }
            _ => self.open_bases(i),
        }
    }

    /// Change the pull requests of review `f`: each it holds picked, the
    /// projects it spans linked once listed.
    pub(super) fn edit(&mut self, f: &Feature) {
        self.link_first = f.links.iter().map(|l| l.project.clone()).collect();
        let Some(review) = self.review.as_mut() else {
            return;
        };
        review.into = Some((f.slug.clone(), f.title.clone()));
        // Its AI review's agent is picked on its page.
        review.reviewers.clear();
        for repo in &f.repos {
            let Some(pull) = f.pull_of(&repo.key()) else {
                continue;
            };
            let known = feature::pr::OpenPull {
                number: pull.number,
                title: String::new(),
                head_ref: pull.head_ref.clone(),
                author: pull.author.clone(),
                draft: false,
                url: pull.url.clone(),
                mine: pull.mine,
            };
            let entry = RepoPulls {
                picked: Some(pull.number),
                chosen: true,
                known: Some(known),
                ..RepoPulls::default()
            };
            review
                .pulls
                .insert((repo.project.clone(), repo.name.clone()), entry);
        }
    }

    /// What reviewing would do, or why it can't, as a sentence.
    fn create_review(&self) -> Result<DialogOutcome, String> {
        let root = match &self.root {
            Finding::Looking => {
                return Err("Still finding the repositories; try again in a moment.".into());
            }
            Finding::Failed(why) => return Err(format!("No repositories to start from: {why}.")),
            Finding::Found(root) => root,
        };
        let review = self.review.as_ref().ok_or("Not a review.")?;
        let mut picks = Vec::new();
        for row in self.rows() {
            let Some(key) = self.row_repo(row) else {
                continue;
            };
            let Some(pull) = review.pulls.get(&key).and_then(RepoPulls::picked_pull) else {
                continue;
            };
            let (Some(found), Some((host, slug, number))) =
                (self.repo_found(&key), crate::ci::pull_request_of(&pull.url))
            else {
                return Err(format!(
                    "Can't tell where #{} of {} is.",
                    pull.number, key.1
                ));
            };
            picks.push(crate::pr_review::Pick {
                project: key.0.clone(),
                repo: key.1.clone(),
                main: found.main.clone(),
                host,
                slug,
                number,
                head_ref: pull.head_ref.clone(),
            });
        }
        if picks.is_empty() {
            return Err("Pick a pull request of a repository (Space).".into());
        }
        if !root.multi && !picks.iter().any(|p| p.project.is_none()) {
            let name = root.repos.first().map_or("", |r| r.name.as_str());
            return Err(format!(
                "Pick a pull request of {name}: the project's only repository."
            ));
        }
        let lit = review.reviewers.iter().find(|c| c.lit).map(|c| &c.name);
        let reviewer = lit.filter(|name| {
            (review.default_reviewer.as_ref()).is_none_or(|d| !d.eq_ignore_ascii_case(name))
        });
        Ok(DialogOutcome::Review {
            title: self.title.trim().to_owned(),
            root: root.clone(),
            links: self.links(),
            picks,
            into: review.into.as_ref().map(|(slug, _)| slug.clone()),
            reviewer: reviewer.cloned(),
        })
    }

    /// The Reviewer row's chips: none, no row.
    pub(super) fn reviewers(&self) -> &[AgentChip] {
        self.review.as_ref().map_or(&[], |r| &r.reviewers)
    }

    /// Where its parts go on an output of `size`.
    pub(super) fn layout(
        &self,
        size: smithay::utils::Size<i32, Logical>,
    ) -> chrome::NewFeatureLayout {
        chrome::new_feature_layout(size, self.slots(), self.error.is_some(), self.reviewers())
    }

    /// Light reviewer chip `i`, one that may review.
    pub(super) fn pick_reviewer(&mut self, i: usize) {
        let Some(review) = self.review.as_mut() else {
            return;
        };
        if review.reviewers.get(i).is_none_or(|c| c.off) {
            return;
        }
        for (k, chip) in review.reviewers.iter_mut().enumerate() {
            chip.lit = k == i;
        }
    }

    /// Light the reviewer chip before (`back`) or after the lit one.
    fn step_reviewer(&mut self, back: bool) {
        let chips = self.reviewers();
        let lit = chips.iter().position(|c| c.lit);
        let to = match (lit, back) {
            (None, _) => Some(0),
            (Some(i), true) => i.checked_sub(1),
            (Some(i), false) => Some(i + 1),
        };
        if let Some(to) = to {
            self.pick_reviewer(to);
        }
    }

    /// The row Alt+B acts on: the one with the keyboard, else the first
    /// repository ticked (in a review: with a pull request picked), else
    /// the first.
    pub(super) fn base_row(&self) -> Option<usize> {
        let rows = self.rows();
        let is_repo = |i: &usize| rows.get(*i).is_some_and(|r| self.row_repo(*r).is_some());
        self.focus.filter(is_repo).or_else(|| {
            // A review's are those with a pull request picked.
            let picked = |row: DialogRow| {
                let key = self.row_repo(row);
                let review = self.review.as_ref();
                key.and_then(|k| review?.pulls.get(&k))
                    .is_some_and(|e| e.picked.is_some())
            };
            let ticked = |i: &usize| match rows[*i] {
                row if self.review.is_some() => picked(row),
                DialogRow::Home(n) => self.touched.get(n) == Some(&true),
                DialogRow::Linked(k, n) => self.others[k].touched.get(n) == Some(&true),
                _ => false,
            };
            (0..rows.len())
                .find(ticked)
                .or_else(|| (0..rows.len()).find(is_repo))
        })
    }

    /// List the branches of row `i`'s repository in place of the rows, the
    /// one it starts from now with the keyboard.
    pub(super) fn open_bases(&mut self, i: usize) {
        let Some(repo) = self.rows().get(i).and_then(|r| self.row_repo(*r)) else {
            return;
        };
        let current = self.repo_found(&repo).map(|r| r.base.clone());
        let picked = (self.review.as_ref())
            .and_then(|r| r.pulls.get(&repo))
            .and_then(|e| e.picked);
        self.choosing = Some(Choosing {
            repo,
            back: (self.focus, self.top),
        });
        self.query.clear();
        let at = match self.review {
            Some(_) => picked.and_then(|n| self.pull_rows().iter().position(|p| p.number == n)),
            None => current.and_then(|c| self.branch_rows().iter().position(|b| *b == c)),
        };
        self.focus = Some(at.unwrap_or(0));
        self.top = 0;
        self.error = None;
    }

    /// Back to the rows it came from.
    pub(super) fn close_bases(&mut self) {
        if let Some(choosing) = self.choosing.take() {
            (self.focus, self.top) = choosing.back;
        }
        self.query.clear();
        self.error = None;
    }

    /// Start the repository being chosen for from branch row `i`.
    pub(super) fn pick_base(&mut self, i: usize) {
        let Some(branch) = self.branch_rows().get(i).cloned() else {
            return;
        };
        let Some(repo) = self.choosing.as_ref().map(|c| c.repo.clone()) else {
            return;
        };
        if let Some(entry) = self.bases.get_mut(&repo) {
            entry.picked = true;
        }
        if let Some(found) = self.repo_found_mut(&repo) {
            found.base = branch;
        }
        self.close_bases();
    }

    /// A key while choosing a base: all but the movement and Enter go to the
    /// filter.
    fn base_key(&mut self, mods: &ModifiersState, sym: Keysym, typed: Keysym) {
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        let alt = mods.alt && !mods.ctrl && !mods.logo;
        let n = self.repos();
        let step = |at: Option<usize>, back: bool| match (at, n) {
            (_, 0) => None,
            (None, _) => Some(0),
            (Some(i), _) if back => Some((i + n - 1) % n),
            (Some(i), _) => Some((i + 1) % n),
        };
        match sym {
            Keysym::Escape if !self.query.is_empty() => {
                self.query.clear();
                self.focus = Some(0);
                self.top = 0;
                self.error = None;
            }
            Keysym::Escape => self.close_bases(),
            Keysym::b | Keysym::B if alt => self.close_bases(),
            Keysym::l | Keysym::L if ctrl && mods.shift => self.pick(),
            Keysym::Return | Keysym::KP_Enter if n == 0 => {
                self.error = Some(match self.review {
                    Some(_) => "No pull request matches that.".into(),
                    None => "No branch matches that.".into(),
                });
            }
            Keysym::Return | Keysym::KP_Enter => {
                // A paste narrows the rows without moving the keyboard.
                let i = self.focus.filter(|i| *i < n).unwrap_or(0);
                match self.review {
                    Some(_) => self.pick_pull(i),
                    None => self.pick_base(i),
                }
            }
            Keysym::Tab if !mods.shift => self.focus = step(self.focus, false),
            Keysym::ISO_Left_Tab | Keysym::Tab => self.focus = step(self.focus, true),
            Keysym::Down | Keysym::KP_Down => self.focus = step(self.focus, false),
            Keysym::Up | Keysym::KP_Up => self.focus = step(self.focus, true),
            Keysym::j | Keysym::J if ctrl => self.focus = step(self.focus, false),
            Keysym::k | Keysym::K if ctrl => self.focus = step(self.focus, true),
            _ if self.query.key(mods, sym, typed) == Edit::Changed => {
                self.focus = (self.repos() > 0).then_some(0);
                self.top = 0;
                self.error = None;
            }
            _ => {}
        }
    }

    /// The field the keys type in: the branch filter while choosing a base.
    pub fn field_mut(&mut self) -> &mut TextField {
        match self.choosing {
            Some(_) => &mut self.query,
            None => &mut self.title,
        }
    }

    /// Whether row `row` can't be ticked: the only home repository, a
    /// project not found.
    pub(super) fn fixed(&self, row: DialogRow) -> bool {
        match row {
            DialogRow::Home(_) => !matches!(&self.root, Finding::Found(root) if root.multi),
            DialogRow::Linked(..) | DialogRow::Branch(_) => false,
            DialogRow::Project(k) => !matches!(self.others[k].root, Finding::Found(_)),
        }
    }

    pub(super) fn toggle(&mut self, i: usize) {
        let Some(row) = self.rows().get(i).copied() else {
            return;
        };
        if self.review.is_some() && self.row_repo(row).is_some() {
            return self.toggle_pull(i);
        }
        if self.fixed(row) {
            return;
        }
        let tick = match row {
            DialogRow::Home(i) => self.touched.get_mut(i),
            DialogRow::Linked(k, i) => self.others[k].touched.get_mut(i),
            DialogRow::Project(k) => Some(&mut self.others[k].linked),
            DialogRow::Branch(_) => None,
        };
        if let Some(t) = tick {
            *t = !*t;
        }
        self.error = None;
    }

    /// The projects ticked and found.
    pub(super) fn links(&self) -> Vec<LinkSpec> {
        self.others
            .iter()
            .filter(|o| o.linked)
            .filter_map(|o| {
                let Finding::Found(root) = &o.root else {
                    return None;
                };
                Some(LinkSpec {
                    project: o.project.clone(),
                    name: o.name.clone(),
                    root: root.clone(),
                    touched: (root.repos.iter().zip(&o.touched))
                        .filter(|(_, t)| **t)
                        .map(|(r, _)| r.name.clone())
                        .collect(),
                })
            })
            .collect()
    }

    /// A project ticked with none of its repositories, as a form error.
    fn unpicked(&self) -> Option<String> {
        let o = (self.others.iter()).find(|o| {
            o.linked && matches!(o.root, Finding::Found(_)) && !o.touched.iter().any(|t| *t)
        })?;
        Some(format!("Tick a repository of {}, or unlink it.", o.name))
    }

    /// What creating it would do, or why it can't, as a sentence.
    pub(super) fn create(&self) -> Result<DialogOutcome, String> {
        if self.review.is_some() {
            return self.create_review();
        }
        if let Some(why) = self.unpicked() {
            return Err(why);
        }
        let title = self.title.trim();
        if title.is_empty() {
            return Err("Give the feature a title.".into());
        }
        let root = match &self.root {
            Finding::Looking => {
                return Err("Still finding the repositories; try again in a moment.".into());
            }
            Finding::Failed(why) => return Err(format!("No repositories to start from: {why}.")),
            Finding::Found(root) => root,
        };
        let touched: Vec<String> = root
            .repos
            .iter()
            .zip(&self.touched)
            .filter(|(_, t)| **t)
            .map(|(r, _)| r.name.clone())
            .collect();
        let links = self.links();
        if touched.is_empty() && links.iter().all(|l| l.touched.is_empty()) {
            return Err("Tick at least one repository the feature changes.".into());
        }
        Ok(DialogOutcome::Create {
            title: title.to_owned(),
            root: root.clone(),
            touched,
            links,
        })
    }

    /// Switch the rows between the repositories and the project picker.
    pub(super) fn pick(&mut self) {
        self.close_bases();
        self.picking = !self.picking;
        // The picker's rows have the keyboard, found yet or not.
        self.focus = self.picking.then_some(0);
        self.top = 0;
        self.error = None;
    }

    /// Move the keyboard between the title, the rows and the Reviewer row.
    fn step(&mut self, back: bool) {
        let n = if self.multi() { self.repos() } else { 0 };
        let reviewer = !self.picking && !self.reviewers().is_empty();
        let stops: Vec<Option<usize>> = (std::iter::once(None))
            .chain((0..n).map(Some))
            .chain(reviewer.then_some(Some(REVIEWER)))
            .collect();
        let at = stops.iter().position(|s| *s == self.focus).unwrap_or(0);
        let k = stops.len();
        self.focus = stops[if back { (at + k - 1) % k } else { (at + 1) % k }];
    }

    /// What `pos` is over: a row with no base button there is only a row.
    pub(super) fn hit(
        &self,
        layout: &chrome::NewFeatureLayout,
        pos: Point<f64, Logical>,
    ) -> Option<NewFeatureHit> {
        match layout.hit(self.top, pos) {
            Some(NewFeatureHit::Base(i))
                if !(self.rows().get(i)).is_some_and(|r| self.row_repo(*r).is_some()) =>
            {
                Some(NewFeatureHit::Repo(i))
            }
            hit => hit,
        }
    }

    /// Scroll so the focused row is among the `fit` shown.
    fn reveal(&mut self, fit: usize) {
        let fit = fit.max(1);
        if let Some(i) = self.focus.filter(|i| *i != REVIEWER) {
            if i < self.top {
                self.top = i;
            } else if i >= self.top + fit {
                self.top = i + 1 - fit;
            }
        }
        self.top = self.top.min(self.repos().saturating_sub(fit));
    }

    /// Handle a key, `fit` rows shown at once: `sym` is the key's own
    /// symbol, `typed` what it types.
    pub(super) fn key(
        &mut self,
        mods: &ModifiersState,
        sym: Keysym,
        typed: Keysym,
        fit: usize,
    ) -> Option<DialogOutcome> {
        let plain = !(mods.ctrl || mods.alt || mods.logo);
        let ctrl = mods.ctrl && !mods.alt && !mods.logo;
        let alt = mods.alt && !mods.ctrl && !mods.logo;
        if self.choosing.is_some() {
            self.base_key(mods, sym, typed);
            self.reveal(fit);
            return None;
        }
        match sym {
            Keysym::Escape => {
                // Out of the picker, back to the title, then the title
                // cleared, then closed.
                if self.picking {
                    self.pick();
                } else if self.focus.is_some() {
                    self.focus = None;
                } else if !self.title.is_empty() {
                    self.title.clear();
                    self.error = None;
                } else {
                    return Some(DialogOutcome::Cancel);
                }
            }
            Keysym::Return | Keysym::KP_Enter => match self.create() {
                Ok(outcome) => return Some(outcome),
                Err(why) => self.error = Some(why),
            },
            Keysym::l | Keysym::L if ctrl && mods.shift => self.pick(),
            Keysym::b | Keysym::B if alt => {
                if let Some(i) = self.base_row() {
                    self.open_bases(i);
                }
            }
            Keysym::Tab if !mods.shift => self.step(false),
            Keysym::ISO_Left_Tab | Keysym::Tab => self.step(true),
            Keysym::Down | Keysym::KP_Down => self.step(false),
            Keysym::Up | Keysym::KP_Up => self.step(true),
            Keysym::j | Keysym::J if ctrl => self.step(false),
            Keysym::k | Keysym::K if ctrl => self.step(true),
            Keysym::Left | Keysym::KP_Left if plain && self.focus == Some(REVIEWER) => {
                self.step_reviewer(true)
            }
            Keysym::Right | Keysym::KP_Right if plain && self.focus == Some(REVIEWER) => {
                self.step_reviewer(false)
            }
            Keysym::space if plain && self.focus.is_some() => {
                if let Some(i) = self.focus {
                    self.toggle(i);
                }
            }
            _ if self.focus.is_none() && self.title.key(mods, sym, typed) == Edit::Changed => {
                self.error = None;
            }
            _ => {}
        }
        self.reveal(fit);
        None
    }
}
