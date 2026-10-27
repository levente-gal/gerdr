//! The issues assigned to the user, for the inbox's todos: GitHub's, per
//! repository of the shown project (`gh issue list --assignee @me`, see
//! `ci::gh_json`), and Linear's (its GraphQL API's `viewer.assignedIssues`,
//! through `curl` with the key on stdin, never in its arguments).
//!
//! Reading blocks: the caller runs `read_all` on a thread of its own.

use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::Instant,
};

use serde::Deserialize;

use crate::{ci, config};

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

/// What a round of reading the issues came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssuesRead {
    /// Per repository, by `ci::key(host, slug)`: its issues, or why they
    /// couldn't be read.
    pub github: Vec<(String, Result<Vec<Issue>, String>)>,
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
    /// Every issue read, GitHub's then Linear's.
    pub fn issues(&self) -> Vec<&Issue> {
        let github = self.last.github.iter().filter_map(|(_, r)| r.as_ref().ok());
        let linear = self.last.linear.iter().filter_map(|r| r.as_ref().ok());
        github.chain(linear).flatten().collect()
    }
}

/// Read the issues assigned to the user: in each of `github`'s
/// repositories, and on Linear with `linear`'s key (none without one).
/// Blocks until every read is done; they run side by side. A repository
/// whose issues can't be read (`gh` missing or signed out) gets the error
/// `gh` gave, and Linear's what it or `curl` said; nothing is logged but at
/// debug level.
pub fn read_all(github: Vec<GhRepo>, linear: Option<config::Linear>) -> IssuesRead {
    std::thread::scope(|scope| {
        let linear = linear.map(|linear| scope.spawn(move || read_linear(&linear)));
        let reads: Vec<_> = github
            .iter()
            .map(|repo| scope.spawn(move || read_github(repo)))
            .collect();
        let github = github
            .iter()
            .zip(reads)
            .map(|(repo, read)| {
                let key = ci::key(&repo.host, &repo.slug);
                let read = joined(read);
                if let Err(err) = &read {
                    tracing::debug!("issues of {key}: {err}");
                }
                (key, read)
            })
            .collect();
        let linear = linear.map(|read| {
            let read = joined(read);
            if let Err(err) = &read {
                tracing::debug!("Linear issues: {err}");
            }
            read
        });
        IssuesRead { github, linear }
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
    fn without_a_key_linear_isnt_read() {
        let read = read_all(Vec::new(), None);
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
                linear: Some(Ok(vec![issue("L")])),
            },
            ..IssuesState::default()
        };
        let keys: Vec<&str> = state.issues().iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, ["1", "2", "3", "L"]);
    }
}
