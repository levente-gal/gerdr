//! The issues assigned to the user, for the inbox's todos: GitHub's, per
//! repository of the shown project (`gh issue list --assignee @me`, see
//! `ci::gh_json`), and Linear's (its GraphQL API's `viewer.assignedIssues`,
//! through `curl` with the key on stdin, never in its arguments). And the
//! tickets of a repository that holds only tickets (`Project::tickets`):
//! its issues assigned to the user, in the order a GitHub project of it
//! places them, by Status column then by place in the column.
//!
//! Reading blocks: the caller runs `read_all` on a thread of its own.

use std::{
    cmp::Reverse,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::Instant,
};

use serde::{Deserialize, Serialize};

use crate::{ci, config, problems};

/// Where the problems reading the issues come from, for
/// `problems::set`: GitHub's issues, the ticket repositories' tickets,
/// their boards' order, and Linear's issues.
pub const GITHUB_PROBLEMS: &str = "inbox:github";
pub const TICKET_PROBLEMS: &str = "inbox:tickets";
pub const BOARD_PROBLEMS: &str = "inbox:board";
pub const LINEAR_PROBLEMS: &str = "inbox:linear";

/// What a board that couldn't be read for `slug`'s tickets says, as a
/// problem: how to give the token the scope it needs, when that is why.
fn board_problem(slug: &str, err: &str) -> String {
    if err.contains("read:project") {
        format!(
            "The tickets of {slug} aren't in their board's order: the GitHub token lacks the \
             read:project scope. Run gh auth refresh -s read:project."
        )
    } else {
        format!("Couldn't read the board of {slug}'s tickets: {err}")
    }
}

/// Where an issue comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IssueSource {
    GitHub,
    Linear,
}

/// An open issue assigned to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub source: IssueSource,
    /// Stable across reads: `<host>/<slug>#<n>` for GitHub, the issue's
    /// uuid for Linear.
    pub id: String,
    /// As shown: `#12`, `ENG-123`.
    pub key: String,
    pub title: String,
    /// Its web page.
    pub url: String,
    /// `open`, or Linear's state's name (`In Progress`).
    pub state: String,
    pub labels: Vec<String>,
    /// When it last changed, as the source gives it (RFC 3339): the
    /// item's fingerprint.
    pub updated: String,
    /// Where it is: the repository's slug, the Linear team's key.
    pub place: String,
}

/// A GitHub repository to read the issues of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhRepo {
    /// Its root on disk, where `gh` runs.
    pub root: PathBuf,
    pub host: String,
    /// `owner/repo`.
    pub slug: String,
}

/// A repository that holds only tickets, as a project file names it:
/// `"owner/repo"`, or `{"repo": "owner/repo", "project": 3}` to take the
/// order of that project of the repository's owner. `host/owner/repo` for
/// a host other than github.com.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "TicketSpec", into = "TicketSpec")]
pub struct TicketRepo {
    /// As written; see `place`.
    pub repo: String,
    /// The project whose order the tickets take, by its number; `None`
    /// for the open project linked to the repository that holds the most
    /// of them.
    pub project: Option<u64>,
}

/// How a project file writes a `TicketRepo`.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum TicketSpec {
    Repo(String),
    Full {
        repo: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<u64>,
    },
}

impl From<TicketSpec> for TicketRepo {
    fn from(spec: TicketSpec) -> Self {
        match spec {
            TicketSpec::Repo(repo) => TicketRepo {
                repo,
                project: None,
            },
            TicketSpec::Full { repo, project } => TicketRepo { repo, project },
        }
    }
}

impl From<TicketRepo> for TicketSpec {
    fn from(repo: TicketRepo) -> Self {
        match repo.project {
            None => TicketSpec::Repo(repo.repo),
            project => TicketSpec::Full {
                repo: repo.repo,
                project,
            },
        }
    }
}

impl TicketRepo {
    /// Its host and `owner/repo`: from `owner/repo` (on github.com) or
    /// `host/owner/repo`, a URL's `https://` and `.git` let be.
    pub fn place(&self) -> Result<(String, String), String> {
        let repo = self.repo.trim();
        let repo = repo
            .strip_prefix("https://")
            .or_else(|| repo.strip_prefix("http://"))
            .unwrap_or(repo);
        let repo = repo.trim_end_matches('/');
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        let parts: Vec<&str> = repo.split('/').collect();
        match parts[..] {
            [owner, name] if !owner.is_empty() && !name.is_empty() => {
                Ok(("github.com".to_owned(), format!("{owner}/{name}")))
            }
            [host, owner, name] if !host.is_empty() && !owner.is_empty() && !name.is_empty() => {
                Ok((host.to_owned(), format!("{owner}/{name}")))
            }
            _ => Err(format!(
                "tickets: \"{}\" is not owner/repo or host/owner/repo",
                self.repo
            )),
        }
    }

    /// What its tickets are kept by: `ci::key(host, slug)`, else as
    /// written.
    pub fn key(&self) -> String {
        self.place()
            .map_or_else(|_| self.repo.clone(), |(host, slug)| ci::key(&host, &slug))
    }
}

/// What a round of reading the issues came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssuesRead {
    /// Per repository, by `ci::key(host, slug)`: its issues, or why they
    /// couldn't be read.
    pub github: Vec<(String, Result<Vec<Issue>, String>)>,
    /// Per ticket repository, by `TicketRepo::key`: its tickets in board
    /// order, or why they couldn't be read.
    pub tickets: Vec<(String, Result<Vec<Issue>, String>)>,
    /// Linear's, or why they couldn't be read; `None` without a key.
    pub linear: Option<Result<Vec<Issue>, String>>,
}

/// The issues as last read, kept in `inbox::Inbox`.
#[derive(Clone, Debug, Default)]
pub struct IssuesState {
    /// The last round's results; a source that failed keeps what it had.
    pub last: IssuesRead,
    /// When the last round ended; `None` before one did.
    pub at: Option<Instant>,
    /// A round is under way.
    pub pending: bool,
}

impl IssuesState {
    /// Every issue read, GitHub's, the tickets, then Linear's.
    pub fn issues(&self) -> Vec<&Issue> {
        let github = self.last.github.iter().filter_map(|(_, r)| r.as_ref().ok());
        let tickets = self
            .last
            .tickets
            .iter()
            .filter_map(|(_, r)| r.as_ref().ok());
        let linear = self.last.linear.iter().filter_map(|r| r.as_ref().ok());
        github.chain(tickets).chain(linear).flatten().collect()
    }
}

/// Read the issues assigned to the user: in each of `github`'s
/// repositories, in each of `tickets` (see `read_tickets`), and on Linear
/// with `linear`'s key (none without one). Blocks until every read is
/// done; they run side by side. A repository whose issues can't be read
/// (`gh` missing or signed out) gets the error `gh` gave, and Linear's what
/// it or `curl` said. Each goes to `problems::set` by its source too
/// (see `GITHUB_PROBLEMS`), a board that couldn't be read as well; a
/// source read without one lets go of its problems.
pub fn read_all(
    github: Vec<GhRepo>,
    tickets: Vec<TicketRepo>,
    linear: Option<config::Linear>,
) -> IssuesRead {
    std::thread::scope(|scope| {
        let linear = linear.map(|linear| scope.spawn(move || read_linear(&linear)));
        let reads: Vec<_> = github
            .iter()
            .map(|repo| scope.spawn(move || read_github(repo)))
            .collect();
        let ticket_reads: Vec<_> = tickets
            .iter()
            .map(|repo| scope.spawn(move || read_tickets(repo)))
            .collect();
        let (mut ticket_problems, mut board_problems) = (Vec::new(), Vec::new());
        let tickets = tickets
            .iter()
            .zip(ticket_reads)
            .map(|(repo, read)| {
                let key = repo.key();
                let read = joined(read).map(|(tickets, board)| {
                    board_problems.extend(board);
                    tickets
                });
                if let Err(err) = &read {
                    ticket_problems.push(format!("Couldn't read the tickets of {key}: {err}"));
                }
                (key, read)
            })
            .collect();
        let mut github_problems = Vec::new();
        let github = github
            .iter()
            .zip(reads)
            .map(|(repo, read)| {
                let key = ci::key(&repo.host, &repo.slug);
                let read = joined(read);
                if let Err(err) = &read {
                    github_problems.push(format!("Couldn't read the issues of {key}: {err}"));
                }
                (key, read)
            })
            .collect();
        let linear = linear.map(joined);
        let linear_problems = match &linear {
            Some(Err(err)) => vec![format!("Couldn't read Linear's issues: {err}")],
            _ => Vec::new(),
        };
        // Two ticket repositories of one board say the same.
        board_problems.dedup();
        problems::set(TICKET_PROBLEMS, ticket_problems);
        problems::set(BOARD_PROBLEMS, board_problems);
        problems::set(GITHUB_PROBLEMS, github_problems);
        problems::set(LINEAR_PROBLEMS, linear_problems);
        IssuesRead {
            github,
            tickets,
            linear,
        }
    })
}

/// What a reading thread came to; a panic is an error.
fn joined<T>(read: std::thread::ScopedJoinHandle<'_, Result<T, String>>) -> Result<T, String> {
    read.join()
        .unwrap_or_else(|_| Err("reading panicked".to_owned()))
}

/// The fields `gh issue list` is asked for.
const GH_FIELDS: &str = "number,title,url,updatedAt,labels,state";

/// The open issues of `repo` assigned to the user.
fn read_github(repo: &GhRepo) -> Result<Vec<Issue>, String> {
    let name = if repo.host == "github.com" {
        repo.slug.clone()
    } else {
        ci::key(&repo.host, &repo.slug)
    };
    let args = [
        "issue",
        "list",
        "--repo",
        &name,
        "--assignee",
        "@me",
        "--state",
        "open",
        "-L",
        "50",
        "--json",
        GH_FIELDS,
    ];
    let found: Vec<GhIssue> = ci::gh_json(&repo.root, &args)?;
    Ok(gh_issues(&repo.host, &repo.slug, found))
}

/// An issue as `gh issue list --json` gives it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhIssue {
    number: u64,
    title: String,
    url: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    labels: Vec<Named>,
    #[serde(default)]
    state: String,
}

/// Anything that has a name: a label, a state, a team (by its `key`).
#[derive(Deserialize)]
struct Named {
    #[serde(alias = "key")]
    name: String,
}

/// The issues of `slug` on `host` out of what `gh issue list --json`
/// printed.
#[cfg(test)]
fn parse_gh_issues(host: &str, slug: &str, json: &str) -> Result<Vec<Issue>, String> {
    let found: Vec<GhIssue> = serde_json::from_str(json).map_err(|err| err.to_string())?;
    Ok(gh_issues(host, slug, found))
}

fn gh_issues(host: &str, slug: &str, found: Vec<GhIssue>) -> Vec<Issue> {
    let repo = ci::key(host, slug);
    found
        .into_iter()
        .map(|issue| Issue {
            source: IssueSource::GitHub,
            id: format!("{repo}#{}", issue.number),
            key: format!("#{}", issue.number),
            title: issue.title,
            url: issue.url,
            state: issue.state.to_lowercase(),
            labels: issue.labels.into_iter().map(|label| label.name).collect(),
            updated: issue.updated_at,
            place: slug.to_owned(),
        })
        .collect()
}

/// The tickets of `repo` assigned to the user (`gh issue list`), in the
/// order of its board (see `board`): by Status column, "No status"
/// first as GitHub shows it, then by place in the column; those on no
/// board after, the most recently changed first. Each one's `state` is its
/// column's name. A board that can't be read (the token without the
/// `read:project` scope) leaves them in that last order, with the problem
/// to say so.
fn read_tickets(repo: &TicketRepo) -> Result<(Vec<Issue>, Option<String>), String> {
    let (host, slug) = repo.place()?;
    // `gh` is told the repository: it needs no checkout to run in.
    let gh = GhRepo {
        root: PathBuf::from("/"),
        host,
        slug,
    };
    let mut tickets = read_github(&gh)?;
    let (board, problem) = match read_board(&gh, repo.project) {
        Ok(board) => (board, None),
        Err(err) => (None, Some(board_problem(&gh.slug, &err))),
    };
    in_board_order(&mut tickets, board.as_ref());
    Ok((tickets, problem))
}

/// What a project's items are asked for: its Status column of each, the
/// columns' order, and the issues assigned to the user in its own order.
const BOARD_FIELDS: &str = "closed \
    field(name:\"Status\"){... on ProjectV2SingleSelectField{options{id}}} \
    items(first:100,query:\"assignee:@me is:open\",orderBy:{field:POSITION,direction:ASC}){nodes{\
    status:fieldValueByName(name:\"Status\"){... on ProjectV2ItemFieldSingleSelectValue{optionId name}} \
    content{... on Issue{number repository{nameWithOwner}}}}}";

/// The board whose order `repo`'s tickets take: project `number` of the
/// repository's owner, else the open project linked to the repository
/// that holds the most of them; `None` without one.
fn read_board(repo: &GhRepo, number: Option<u64>) -> Result<Option<Board>, String> {
    let (owner, name) = repo.slug.split_once('/').unwrap_or((&repo.slug, ""));
    let (query, var) = match number {
        Some(number) => (
            format!(
                "query($owner:String!,$number:Int!){{repositoryOwner(login:$owner){{\
                 ... on ProjectV2Owner{{projectV2(number:$number){{{BOARD_FIELDS}}}}}}}}}"
            ),
            ["-F", &format!("number={number}")].map(str::to_owned),
        ),
        None => (
            format!(
                "query($owner:String!,$name:String!){{repository(owner:$owner,name:$name){{\
                 projectsV2(first:20){{nodes{{{BOARD_FIELDS}}}}}}}}}"
            ),
            ["-f", &format!("name={name}")].map(str::to_owned),
        ),
    };
    let query = format!("query={query}");
    let owner = format!("owner={owner}");
    let mut args = vec![
        "api", "graphql", "-f", &query, "-f", &owner, &var[0], &var[1],
    ];
    if repo.host != "github.com" {
        args.extend(["--hostname", &repo.host]);
    }
    let reply: Gql<BoardData> = ci::gh_json(&repo.root, &args)?;
    let linked = reply.data.repository.map(|r| r.projects_v2.nodes);
    let owned = reply.data.repository_owner.and_then(|o| o.project_v2);
    let found = linked
        .unwrap_or_default()
        .into_iter()
        .chain(owned)
        .collect();
    Ok(board(found, &repo.slug, number.is_some()))
}

/// A project's order of the user's tickets.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Board {
    /// Its Status columns' option ids, in order.
    columns: Vec<String>,
    /// The tickets on it, in its order: their number, their column's
    /// option id and name (`None` for "No status").
    items: Vec<(u64, Option<(String, String)>)>,
}

/// The board of `slug`'s tickets out of `found`: the only one when
/// `chosen`, else the open one holding the most of them (the first of
/// those tied); `None` when none holds any.
fn board(found: Vec<GhProject>, slug: &str, chosen: bool) -> Option<Board> {
    let boards = found
        .into_iter()
        .filter(|project| chosen || !project.closed)
        .map(|project| Board {
            columns: project
                .field
                .map(|f| f.options.into_iter().map(|o| o.id).collect())
                .unwrap_or_default(),
            items: project
                .items
                .nodes
                .into_iter()
                .filter_map(|item| {
                    let content = item.content?;
                    let of = content.repository?.name_with_owner;
                    if !of.eq_ignore_ascii_case(slug) {
                        return None;
                    }
                    let status = item.status.and_then(|s| Some((s.option_id?, s.name?)));
                    Some((content.number?, status))
                })
                .collect(),
        });
    let mut best: Option<Board> = None;
    for board in boards {
        if best
            .as_ref()
            .is_none_or(|b| board.items.len() > b.items.len())
        {
            best = Some(board);
        }
    }
    best.filter(|b| !b.items.is_empty())
}

/// Put `tickets` in `board`'s order (see `read_tickets`), each on it
/// stated as its column.
fn in_board_order(tickets: &mut [Issue], board: Option<&Board>) {
    let number = |issue: &Issue| issue.key.trim_start_matches('#').parse::<u64>().ok();
    let place = |issue: &Issue| {
        let board = board?;
        let n = number(issue)?;
        let at = board.items.iter().position(|(m, _)| *m == n)?;
        let column = match &board.items[at].1 {
            None => 0,
            Some((id, _)) => {
                1 + board
                    .columns
                    .iter()
                    .position(|c| c == id)
                    .unwrap_or(board.columns.len())
            }
        };
        Some((column, at))
    };
    for issue in tickets.iter_mut() {
        if let (Some(board), Some(n)) = (board, number(issue))
            && let Some((_, status)) = board.items.iter().find(|(m, _)| *m == n)
        {
            issue.state = status
                .as_ref()
                .map_or_else(|| "No status".to_owned(), |(_, name)| name.clone());
        }
    }
    // Off the board last; RFC 3339 times of one source sort as text.
    tickets.sort_by(|a, b| {
        let key = |i: &Issue| place(i).map_or((1, 0, 0), |(c, at)| (0, c, at));
        key(a)
            .cmp(&key(b))
            .then_with(|| Reverse(&a.updated).cmp(&Reverse(&b.updated)))
    });
}

/// A GraphQL reply's data.
#[derive(Deserialize)]
struct Gql<T> {
    data: T,
}

/// The projects linked to the repository, or the one project asked for.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BoardData {
    repository: Option<LinkedProjects>,
    repository_owner: Option<OwnerProject>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OwnerProject {
    project_v2: Option<GhProject>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkedProjects {
    projects_v2: Nodes<GhProject>,
}

#[derive(Deserialize)]
struct GhProject {
    #[serde(default)]
    closed: bool,
    /// `{}` when its Status isn't a single select, `null` without one.
    field: Option<StatusField>,
    items: Nodes<GhItem>,
}

#[derive(Deserialize)]
struct StatusField {
    #[serde(default)]
    options: Vec<OptionId>,
}

#[derive(Deserialize)]
struct OptionId {
    id: String,
}

#[derive(Deserialize)]
struct GhItem {
    status: Option<StatusValue>,
    /// `{}` for a pull request or a draft.
    content: Option<GhContent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusValue {
    option_id: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct GhContent {
    number: Option<u64>,
    repository: Option<NameWithOwner>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameWithOwner {
    name_with_owner: String,
}

/// Linear's GraphQL endpoint.
const LINEAR_API: &str = "https://api.linear.app/graphql";

/// The open issues assigned to the user, most recently changed first; with
/// `$teams`, only those teams'.
const LINEAR_QUERY: &str = "query($teams:[String!]){viewer{assignedIssues(first:50,orderBy:updatedAt,filter:{state:{type:{nin:[\"completed\",\"canceled\"]}}TEAMS}){nodes{id identifier title url updatedAt state{name} team{key} labels{nodes{name}}}}}}";

/// The JSON body of the request for the issues assigned to the user in
/// `teams` (every team's when empty).
pub fn linear_body(teams: &[String]) -> String {
    let (query, variables) = if teams.is_empty() {
        (
            LINEAR_QUERY
                .replace("($teams:[String!])", "")
                .replace("TEAMS", ""),
            serde_json::json!({}),
        )
    } else {
        (
            LINEAR_QUERY.replace("TEAMS", ",team:{key:{in:$teams}}"),
            serde_json::json!({ "teams": teams }),
        )
    };
    serde_json::json!({ "query": query, "variables": variables }).to_string()
}

/// The open Linear issues assigned to the user, through `curl`: its
/// config, the key included, goes on its stdin.
fn read_linear(linear: &config::Linear) -> Result<Vec<Issue>, String> {
    let config = curl_config(&linear.key, &linear_body(&linear.teams))?;
    let mut child = Command::new("curl")
        .args([
            "-sS",
            "--fail-with-body",
            "--max-time",
            "15",
            "--config",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => "curl is not installed".to_owned(),
            _ => format!("curl: {err}"),
        })?;
    // Written whole before reading: the config is small, so curl takes it
    // all in before it has anything to print.
    let wrote = child
        .stdin
        .take()
        .map(|mut stdin| stdin.write_all(config.as_bytes()));
    let out = child
        .wait_with_output()
        .map_err(|err| format!("curl: {err}"))?;
    if let Some(Err(err)) = wrote {
        return Err(format!("curl: {err}"));
    }
    let body = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        // An HTTP error's body (`--fail-with-body`) says what Linear
        // objected to; else curl does.
        if let Ok(reply) = serde_json::from_str::<LinearReply>(&body)
            && !reply.errors.is_empty()
        {
            return Err(linear_errors(&reply.errors));
        }
        let err = String::from_utf8_lossy(&out.stderr);
        let why = err.lines().map(str::trim).find(|l| !l.is_empty());
        return Err(why.map_or_else(|| "curl failed".to_owned(), str::to_owned));
    }
    parse_linear(&body)
}

/// A config for `curl --config -` that posts `body` to Linear with `key`.
/// Values are quoted, with `\` and `"` escaped; one with a line break or
/// another control character in it can't be, and is refused.
pub fn curl_config(key: &str, body: &str) -> Result<String, String> {
    let quoted = |value: &str| -> Result<String, String> {
        if value.chars().any(char::is_control) {
            return Err("a control character can't go in curl's config".to_owned());
        }
        Ok(format!(
            "\"{}\"",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        ))
    };
    let key = quoted(&format!("Authorization: {}", key.trim()))
        .map_err(|_| "the Linear API key has a control character in it".to_owned())?;
    Ok(format!(
        "url = {}\nheader = {key}\nheader = {}\ndata-binary = {}\n",
        quoted(LINEAR_API)?,
        quoted("Content-Type: application/json")?,
        quoted(body)?,
    ))
}

/// A GraphQL reply.
#[derive(Deserialize)]
struct LinearReply {
    data: Option<LinearData>,
    #[serde(default)]
    errors: Vec<LinearError>,
}

#[derive(Deserialize)]
struct LinearError {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct LinearData {
    viewer: LinearViewer,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinearViewer {
    assigned_issues: Nodes<LinearIssue>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinearIssue {
    id: String,
    identifier: String,
    title: String,
    url: String,
    #[serde(default)]
    updated_at: String,
    state: Option<Named>,
    team: Option<Named>,
    labels: Option<Nodes<Named>>,
}

/// What Linear's `errors` say, as one line.
fn linear_errors(errors: &[LinearError]) -> String {
    let messages: Vec<&str> = errors.iter().map(|e| e.message.as_str()).collect();
    format!("Linear: {}", messages.join("; "))
}

/// The issues in Linear's reply `json`, or the errors it gave.
pub fn parse_linear(json: &str) -> Result<Vec<Issue>, String> {
    let reply: LinearReply =
        serde_json::from_str(json).map_err(|err| format!("Linear's reply: {err}"))?;
    if !reply.errors.is_empty() {
        return Err(linear_errors(&reply.errors));
    }
    let data = reply
        .data
        .ok_or_else(|| "Linear's reply has no data".to_owned())?;
    Ok(data
        .viewer
        .assigned_issues
        .nodes
        .into_iter()
        .map(|issue| Issue {
            source: IssueSource::Linear,
            id: issue.id,
            key: issue.identifier,
            title: issue.title,
            url: issue.url,
            state: issue.state.map(|state| state.name).unwrap_or_default(),
            labels: issue
                .labels
                .map(|labels| labels.nodes.into_iter().map(|l| l.name).collect())
                .unwrap_or_default(),
            updated: issue.updated_at,
            place: issue.team.map(|team| team.name).unwrap_or_default(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh_issues_are_read_from_its_json() {
        let json = r#"[
            {"number": 12, "title": "Crash on start", "url": "https://github.com/me/app/issues/12",
             "updatedAt": "2026-10-20T08:00:00Z", "state": "OPEN",
             "labels": [{"id": "L1", "name": "bug", "description": "", "color": "d73a4a"},
                        {"id": "L2", "name": "p1", "description": "", "color": "000000"}]},
            {"number": 3, "title": "Docs", "url": "https://github.com/me/app/issues/3",
             "updatedAt": "2026-09-01T10:00:00Z", "state": "OPEN", "labels": []}
        ]"#;
        let issues = parse_gh_issues("github.com", "me/app", json).unwrap();
        assert_eq!(
            issues[0],
            Issue {
                source: IssueSource::GitHub,
                id: "github.com/me/app#12".to_owned(),
                key: "#12".to_owned(),
                title: "Crash on start".to_owned(),
                url: "https://github.com/me/app/issues/12".to_owned(),
                state: "open".to_owned(),
                labels: vec!["bug".to_owned(), "p1".to_owned()],
                updated: "2026-10-20T08:00:00Z".to_owned(),
                place: "me/app".to_owned(),
            }
        );
        assert_eq!(issues[1].id, "github.com/me/app#3");
        assert!(issues[1].labels.is_empty());
        assert_eq!(
            parse_gh_issues("github.com", "me/app", "[]"),
            Ok(Vec::new())
        );
        assert!(parse_gh_issues("github.com", "me/app", "nope").is_err());
    }

    const LINEAR_OK: &str = r#"{"data": {"viewer": {"assignedIssues": {"nodes": [
        {"id": "9b2c1f3e-0000-4000-8000-000000000001", "identifier": "ENG-123",
         "title": "Speed up sync", "url": "https://linear.app/acme/issue/ENG-123/speed-up-sync",
         "updatedAt": "2026-10-26T12:34:56.000Z", "state": {"name": "In Progress"},
         "team": {"key": "ENG"}, "labels": {"nodes": [{"name": "Perf"}]}},
        {"id": "9b2c1f3e-0000-4000-8000-000000000002", "identifier": "OPS-7",
         "title": "Rotate keys", "url": "https://linear.app/acme/issue/OPS-7",
         "updatedAt": "2026-10-01T00:00:00.000Z", "state": {"name": "Todo"},
         "team": {"key": "OPS"}, "labels": {"nodes": []}}
    ]}}}}"#;

    #[test]
    fn linear_issues_are_read_from_its_reply() {
        let issues = parse_linear(LINEAR_OK).unwrap();
        assert_eq!(
            issues[0],
            Issue {
                source: IssueSource::Linear,
                id: "9b2c1f3e-0000-4000-8000-000000000001".to_owned(),
                key: "ENG-123".to_owned(),
                title: "Speed up sync".to_owned(),
                url: "https://linear.app/acme/issue/ENG-123/speed-up-sync".to_owned(),
                state: "In Progress".to_owned(),
                labels: vec!["Perf".to_owned()],
                updated: "2026-10-26T12:34:56.000Z".to_owned(),
                place: "ENG".to_owned(),
            }
        );
        assert_eq!(issues[1].key, "OPS-7");
        assert_eq!(issues[1].place, "OPS");
    }

    #[test]
    fn linear_errors_are_errors() {
        let json = r#"{"errors": [
            {"message": "Authentication required, not authenticated",
             "extensions": {"type": "authentication error", "code": "AUTHENTICATION_ERROR"}},
            {"message": "Second"}
        ]}"#;
        assert_eq!(
            parse_linear(json),
            Err("Linear: Authentication required, not authenticated; Second".to_owned())
        );
        // Even beside data.
        let json = r#"{"data": null, "errors": [{"message": "Bad filter"}]}"#;
        assert_eq!(parse_linear(json), Err("Linear: Bad filter".to_owned()));
        assert!(parse_linear(r#"{"data": null}"#).is_err());
        assert!(parse_linear("<html>").is_err());
    }

    #[test]
    fn the_linear_query_filters_teams_only_when_asked() {
        let body: serde_json::Value = serde_json::from_str(&linear_body(&[])).unwrap();
        let query = body["query"].as_str().unwrap();
        assert!(!query.contains("$teams"), "{query}");
        assert!(query.starts_with("query{viewer{assignedIssues("), "{query}");
        assert!(query.contains(r#"nin:["completed","canceled"]"#), "{query}");
        assert_eq!(body["variables"], serde_json::json!({}));

        let teams = ["ENG".to_owned(), "OPS".to_owned()];
        let body: serde_json::Value = serde_json::from_str(&linear_body(&teams)).unwrap();
        let query = body["query"].as_str().unwrap();
        assert!(query.starts_with("query($teams:[String!]){"), "{query}");
        assert!(query.contains("team:{key:{in:$teams}}"), "{query}");
        assert_eq!(
            body["variables"],
            serde_json::json!({"teams": ["ENG", "OPS"]})
        );
    }

    #[test]
    fn curl_config_quotes_its_values() {
        let config = curl_config(" lin_api_k\"e\\y\n", r#"{"q":"a \"b\" \\n"}"#);
        // The key's line break is trimmed off; inner ones are refused.
        let config = config.unwrap();
        assert_eq!(
            config,
            concat!(
                "url = \"https://api.linear.app/graphql\"\n",
                "header = \"Authorization: lin_api_k\\\"e\\\\y\"\n",
                "header = \"Content-Type: application/json\"\n",
                "data-binary = \"{\\\"q\\\":\\\"a \\\\\\\"b\\\\\\\" \\\\\\\\n\\\"}\"\n",
            )
        );
        assert!(curl_config("lin\napi", "{}").is_err());
        assert!(curl_config("key", "{\n}").is_err());
        // The whole body, query and all, quotes.
        assert!(curl_config("key", &linear_body(&["ENG".to_owned()])).is_ok());
    }

    #[test]
    fn a_board_without_the_scope_says_how_to_get_it() {
        let scope = "your token has not been granted the required scopes: ['read:project']";
        assert!(board_problem("o/t", scope).contains("gh auth refresh -s read:project"));
        let other = board_problem("o/t", "HTTP 502");
        assert_eq!(other, "Couldn't read the board of o/t's tickets: HTTP 502");
    }

    #[test]
    fn without_a_key_linear_isnt_read() {
        let read = read_all(Vec::new(), Vec::new(), None);
        assert_eq!(read, IssuesRead::default());
        assert_eq!(read.linear, None);
    }

    #[test]
    fn issues_flattens_what_was_read() {
        let issue = |key: &str| Issue {
            source: IssueSource::GitHub,
            id: key.to_owned(),
            key: key.to_owned(),
            title: String::new(),
            url: String::new(),
            state: String::new(),
            labels: Vec::new(),
            updated: String::new(),
            place: String::new(),
        };
        let state = IssuesState {
            last: IssuesRead {
                github: vec![
                    ("a".to_owned(), Ok(vec![issue("1"), issue("2")])),
                    ("b".to_owned(), Err("gh is not installed".to_owned())),
                    ("c".to_owned(), Ok(vec![issue("3")])),
                ],
                tickets: vec![("t".to_owned(), Ok(vec![issue("T")]))],
                linear: Some(Ok(vec![issue("L")])),
            },
            ..IssuesState::default()
        };
        let keys: Vec<&str> = state.issues().iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, ["1", "2", "3", "T", "L"]);
    }

    #[test]
    fn ticket_repos_are_written_short_or_with_their_project() {
        let read: Vec<TicketRepo> =
            serde_json::from_str(r#"["o/t", {"repo": "ghe.example.com/o/t", "project": 3}]"#)
                .unwrap();
        assert_eq!(read[0].project, None);
        assert_eq!(read[1].project, Some(3));
        assert_eq!(read[0].key(), "github.com/o/t");
        assert_eq!(read[1].key(), "ghe.example.com/o/t");
        assert_eq!(
            serde_json::to_string(&read).unwrap(),
            r#"["o/t",{"repo":"ghe.example.com/o/t","project":3}]"#
        );
        let place = |repo: &str| {
            TicketRepo {
                repo: repo.into(),
                project: None,
            }
            .place()
        };
        assert_eq!(
            place("https://github.com/o/t.git/"),
            Ok(("github.com".into(), "o/t".into()))
        );
        assert!(place("t").is_err());
        assert!(place("o/").is_err());
        assert_eq!(
            place("t").err(),
            Some(r#"tickets: "t" is not owner/repo or host/owner/repo"#.into())
        );
    }

    /// A board's reply: project 1, closed, holds #1 only; project 2 holds
    /// #4 (no status), #2 (Done), #3 (In progress) and a pull request and
    /// an issue of another repository.
    const BOARD_REPLY: &str = r#"{"data": {"repository": {"projectsV2": {"nodes": [
        {"closed": true, "field": {"options": [{"id": "a"}]},
         "items": {"nodes": [
            {"status": null, "content": {"number": 1, "repository": {"nameWithOwner": "o/t"}}}]}},
        {"closed": false, "field": {"options": [{"id": "todo"}, {"id": "doing"}, {"id": "done"}]},
         "items": {"nodes": [
            {"status": {"optionId": "done", "name": "Done"},
             "content": {"number": 2, "repository": {"nameWithOwner": "O/T"}}},
            {"status": {}, "content": {"number": 4, "repository": {"nameWithOwner": "o/t"}}},
            {"status": {"optionId": "doing", "name": "In progress"}, "content": {}},
            {"status": {"optionId": "doing", "name": "In progress"},
             "content": {"number": 9, "repository": {"nameWithOwner": "o/other"}}},
            {"status": {"optionId": "doing", "name": "In progress"},
             "content": {"number": 3, "repository": {"nameWithOwner": "o/t"}}}]}}
    ]}}}}"#;

    #[test]
    fn tickets_take_their_boards_order() {
        let reply: Gql<BoardData> = serde_json::from_str(BOARD_REPLY).unwrap();
        let found = reply.data.repository.unwrap().projects_v2.nodes;
        let board = board(found, "o/t", false).unwrap();
        assert_eq!(board.columns, ["todo", "doing", "done"]);
        let numbers: Vec<u64> = board.items.iter().map(|(n, _)| *n).collect();
        assert_eq!(numbers, [2, 4, 3]);

        let ticket = |n: u64, updated: &str| Issue {
            source: IssueSource::GitHub,
            id: format!("github.com/o/t#{n}"),
            key: format!("#{n}"),
            title: String::new(),
            url: String::new(),
            state: "open".into(),
            labels: Vec::new(),
            updated: updated.into(),
            place: "o/t".into(),
        };
        let mut tickets = vec![
            ticket(1, "2026-10-01T00:00:00Z"),
            ticket(2, "2026-10-01T00:00:00Z"),
            ticket(3, "2026-10-01T00:00:00Z"),
            ticket(4, "2026-10-01T00:00:00Z"),
            ticket(5, "2026-10-05T00:00:00Z"),
        ];
        in_board_order(&mut tickets, Some(&board));
        let shown: Vec<(&str, &str)> = tickets
            .iter()
            .map(|t| (t.key.as_str(), t.state.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("#4", "No status"),
                ("#3", "In progress"),
                ("#2", "Done"),
                ("#5", "open"),
                ("#1", "open"),
            ]
        );
        // Without a board, the most recently changed first.
        in_board_order(&mut tickets, None);
        assert_eq!(tickets[0].key, "#5");
    }

    #[test]
    fn a_chosen_board_counts_even_closed() {
        let projects = || {
            let reply: Gql<BoardData> = serde_json::from_str(BOARD_REPLY).unwrap();
            let mut found = reply.data.repository.unwrap().projects_v2.nodes;
            found.truncate(1);
            found
        };
        assert_eq!(board(projects(), "o/t", false), None);
        assert_eq!(
            board(projects(), "o/t", true).map(|b| b.items.len()),
            Some(1)
        );
        let owned = r#"{"data": {"repositoryOwner": {"projectV2": null}}}"#;
        let reply: Gql<BoardData> = serde_json::from_str(owned).unwrap();
        assert!(reply.data.repository.is_none());
        assert!(reply.data.repository_owner.unwrap().project_v2.is_none());
    }
}
