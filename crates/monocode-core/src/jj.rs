//! Native Jujutsu operations. Never mutate a colocated repository through Git.
use crate::scm::{ChangedFile, DiffIndex, History, HistoryCommit, HistoryRef};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub fn root(cwd: &Path) -> Option<PathBuf> {
    let cwd = std::fs::canonicalize(cwd).ok()?;
    for path in cwd.ancestors() {
        if path.join(".jj").is_dir() {
            return Some(path.to_owned());
        }
        // A nested Git repository is not part of the enclosing jj workspace.
        if path.join(".git").exists() {
            return None;
        }
    }
    None
}
pub fn is_workspace(cwd: &Path) -> bool {
    root(cwd).is_some()
}

pub fn run(cwd: &Path, args: &[&str]) -> Result<String, String> {
    String::from_utf8(bytes(cwd, args)?).map_err(|e| e.to_string())
}
fn bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("jj");
    crate::process::configure_child(&mut cmd);
    let output = cmd
        .current_dir(cwd)
        .env("PATH", crate::scm::terminal_search_path())
        .args(["--no-pager", "--color=never"])
        .args(args)
        .output()
        .map_err(|e| format!("Could not run jj: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().into());
    }
    Ok(output.stdout)
}
fn records(text: &str) -> Result<Vec<Value>, String> {
    text.lines()
        .filter(|s| !s.trim().is_empty())
        .map(|s| serde_json::from_str(s).map_err(|e| format!("Invalid jj output: {e}")))
        .collect()
}
pub fn log_value(cwd: &Path, revision: &str, template: &str) -> Result<String, String> {
    run(cwd, &["log", "--no-graph", "-r", revision, "-T", template])
}
pub fn diff_index(cwd: &Path) -> Result<DiffIndex, String> {
    revision_index(cwd, "@")
}
pub fn revision_index(cwd: &Path, revision: &str) -> Result<DiffIndex, String> {
    let root = root(cwd).ok_or("Not a jj workspace")?;
    let canonical = std::fs::canonicalize(cwd).map_err(|e| e.to_string())?;
    let stats = records(&log_value(
        &root,
        revision,
        r#"diff.stat().files().map(|f| json({"path" => f.path(), "additions" => f.lines_added(), "deletions" => f.lines_removed()}) ++ "\n").join("")"#,
    )?)?;
    let entries = records(&run(
        &root,
        &[
            "diff",
            "-r",
            revision,
            "-T",
            r#"json({"path" => path, "status" => status}) ++ "\n""#,
        ],
    )?)?;
    let mut files = Vec::new();
    for entry in entries {
        let name = entry["path"].as_str().ok_or("Missing jj path")?;
        let path = root.join(name);
        let Ok(relative) = path.strip_prefix(&canonical) else {
            continue;
        };
        let stat = stats.iter().find(|s| s["path"] == entry["path"]);
        files.push(ChangedFile {
            path: path.to_string_lossy().into(),
            relative: relative.to_string_lossy().into(),
            status: match entry["status"].as_str().unwrap_or("modified") {
                "removed" => "deleted",
                other => other,
            }
            .into(),
            additions: stat.and_then(|s| s["additions"].as_i64()).unwrap_or(0),
            deletions: stat.and_then(|s| s["deletions"].as_i64()).unwrap_or(0),
            staged: false,
            unstaged: true,
        });
    }
    Ok(DiffIndex {
        branch: Some(log_value(
            &root,
            "@",
            r#"coalesce(bookmarks.join(", "), change_id.short())"#,
        )?),
        head: Some(log_value(&root, revision, "commit_id")?),
        additions: files.iter().map(|f| f.additions).sum(),
        deletions: files.iter().map(|f| f.deletions).sum(),
        files,
        ..Default::default()
    })
}
pub fn history(cwd: &Path, limit: Option<u32>) -> Result<History, String> {
    let output = run(
        cwd,
        &[
            "log",
            "--no-graph",
            "-n",
            &limit.unwrap_or(200).clamp(1, 500).to_string(),
            "-T",
            r#"json({"sha" => commit_id, "shortSha" => change_id.short(), "parents" => parents.map(|p| p.commit_id()), "author" => author.name(), "timestamp" => author.timestamp().format("%s"), "subject" => description.first_line(), "head" => current_working_copy, "bookmarks" => bookmarks.map(|b| b.name())}) ++ "\n""#,
        ],
    )?;
    let mut commits = Vec::new();
    for mut row in records(&output)? {
        row["timestamp"] = row["timestamp"]
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
            .into();
        let refs: Vec<HistoryRef> = row["bookmarks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| b.as_str())
            .map(|name| HistoryRef {
                name: name.into(),
                kind: "local".into(),
            })
            .collect();
        row["refs"] = serde_json::to_value(refs).map_err(|e| e.to_string())?;
        commits.push(serde_json::from_value::<HistoryCommit>(row).map_err(|e| e.to_string())?);
    }
    Ok(History {
        head: Some(log_value(cwd, "@", "commit_id")?),
        commits,
    })
}
pub fn file(cwd: &Path, revision: &str, relative: &str) -> Result<Vec<u8>, String> {
    let pathset = format!(
        "file:{}",
        serde_json::to_string(relative).map_err(|e| e.to_string())?
    );
    let listing = run(cwd, &["file", "list", "-r", revision, "--", &pathset])?;
    if listing.trim().is_empty() {
        return Ok(Vec::new());
    }
    bytes(cwd, &["file", "show", "-r", revision, "--", &pathset])
}
pub fn no_staging(cwd: &Path) -> Result<(), String> {
    if is_workspace(cwd) {
        Err("jj tracks the working copy automatically. Commit changes directly; staging is not available.".into())
    } else {
        Ok(())
    }
}
pub fn commit(cwd: &Path, message: &str, amend: bool) -> Result<(), String> {
    if message.trim().is_empty() {
        return Err("Enter a change description".into());
    }
    run(
        cwd,
        &[if amend { "describe" } else { "commit" }, "-m", message],
    )
    .map(|_| ())
}
pub fn discard(cwd: &Path, relative: Option<&str>) -> Result<(), String> {
    let pathset = relative.map(|p| format!("file:{}", serde_json::to_string(p).unwrap()));
    let mut args = vec!["restore", "--from", "@-", "--"];
    args.push(pathset.as_deref().unwrap_or("."));
    run(cwd, &args).map(|_| ())
}
pub fn workspaces(cwd: &Path) -> Result<Vec<crate::git::Worktree>, String> {
    let rows = records(&run(
        cwd,
        &[
            "workspace",
            "list",
            "-T",
            r#"json({"name" => name, "path" => root, "head" => target.commit_id()}) ++ "\n""#,
        ],
    )?)?;
    rows.into_iter()
        .map(|row| {
            let path = row["path"]
                .as_str()
                .ok_or("jj workspace has no recorded path; update jj workspace metadata first")?
                .to_owned();
            Ok(crate::git::Worktree {
                missing: !Path::new(&path).is_dir(),
                path,
                branch: row["name"].as_str().map(str::to_owned),
                head: row["head"].as_str().unwrap_or_default().into(),
                main: row["name"] == "default",
            })
        })
        .collect()
}
pub fn snapshot(cwd: &Path) -> Result<crate::git::Snapshot, String> {
    let branches = records(&run(
        cwd,
        &[
            "bookmark",
            "list",
            "-T",
            r#"json({"name" => name}) ++ "\n""#,
        ],
    )?)?
    .into_iter()
    .filter_map(|row| {
        Some(crate::git::Branch {
            name: row["name"].as_str()?.into(),
            remote: None,
        })
    })
    .collect();
    Ok(crate::git::Snapshot {
        current: Some(log_value(
            cwd,
            "@",
            r#"coalesce(bookmarks.join(", "), change_id.short())"#,
        )?),
        detached: false,
        linked: false,
        branches,
        worktrees: workspaces(cwd)?,
    })
}
pub fn create_workspace(
    cwd: &Path,
    name: &str,
    base: Option<&str>,
) -> Result<crate::git::Worktree, String> {
    if name.is_empty() || name.starts_with('-') {
        return Err("Enter a workspace name".into());
    }
    let root = root(cwd).ok_or("Not a jj workspace")?;
    let parent = root.with_file_name(format!(
        "{}-workspaces",
        root.file_name().unwrap_or_default().to_string_lossy()
    ));
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let path = parent.join(slug);
    if path.exists() {
        return Err("Workspace path already exists".into());
    }
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let base = base.filter(|s| *s != "HEAD").unwrap_or("@");
    run(
        cwd,
        &[
            "workspace",
            "add",
            "--name",
            name,
            "-r",
            base,
            "--",
            &path.to_string_lossy(),
        ],
    )?;
    workspaces(cwd)?
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(name))
        .ok_or("Created workspace was not found".into())
}

/// Push one unambiguous bookmark attached to the working change or its parent.
pub fn push(cwd: &Path) -> Result<(), String> {
    let mut names = log_value(cwd, "@", r#"bookmarks.map(|b| b.name()).join("\n")"#)?;
    if names.trim().is_empty() {
        names = log_value(cwd, "@-", r#"bookmarks.map(|b| b.name()).join("\n")"#)?;
    }
    let names: Vec<_> = names.lines().filter(|s| !s.is_empty()).collect();
    if names.len() != 1 {
        return Err("Choose exactly one bookmark on the working change or its parent before pushing with jj.".into());
    }
    run(cwd, &["git", "push", "--bookmark", names[0]]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jj_native_workflow() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo");
        run(temp.path(), &["git", "init", "--", path.to_str().unwrap()]).unwrap();
        run(&path, &["config", "set", "--repo", "user.name", "Test"]).unwrap();
        run(
            &path,
            &["config", "set", "--repo", "user.email", "test@example.com"],
        )
        .unwrap();
        run(
            &path,
            &["config", "set", "--repo", "signing.behavior", "drop"],
        )
        .unwrap();
        std::fs::write(path.join("existing"), "before\n").unwrap();
        crate::scm::commit(&path, "base", false).unwrap();
        std::fs::write(path.join("existing"), "after\n").unwrap();
        let name = "space and \"quote\".txt";
        std::fs::write(path.join(name), "new\n").unwrap();
        let index = crate::scm::diff_index(&path).unwrap();
        assert_eq!(index.files.len(), 2);
        assert_eq!((index.additions, index.deletions), (2, 1));
        assert!(index.files.iter().all(|f| !f.staged && f.unstaged));
        assert!(crate::scm::stage_all(&path).is_err());
        let diff = crate::scm::file_diff(&path, "existing", false).unwrap();
        assert_eq!(
            (diff.original.as_str(), diff.current.as_str()),
            ("before\n", "after\n")
        );
        assert_eq!(
            crate::scm::file_diff(&path, name, false).unwrap().current,
            "new\n"
        );
        crate::scm::discard_file(&path, name).unwrap();
        assert!(!path.join(name).exists());
        crate::scm::commit(&path, "change", false).unwrap();
        assert!(crate::scm::diff_index(&path).unwrap().files.is_empty());
        let history = crate::scm::history(&path, Some(5)).unwrap();
        let change = history
            .commits
            .iter()
            .find(|c| c.subject == "change")
            .unwrap();
        assert_eq!(
            crate::scm::commit_files(&path, &change.sha).unwrap().len(),
            1
        );
        assert_eq!(
            crate::scm::commit_file_diff(&path, &change.sha, "existing")
                .unwrap()
                .original,
            "before\n"
        );
        crate::git::create_branch(&path, "feature").unwrap();
        assert!(crate::git::snapshot(&path)
            .unwrap()
            .branches
            .iter()
            .any(|b| b.name == "feature"));
        let workspace = crate::git::create_named_worktree(&path, "second").unwrap();
        assert!(is_workspace(Path::new(&workspace.path)));
        assert_eq!(crate::git::worktrees(&path).unwrap().len(), 2);
        // Nested Git projects must not inherit the outer jj backend.
        let nested = path.join("nested");
        std::fs::create_dir_all(nested.join(".git")).unwrap();
        assert!(!is_workspace(&nested));
    }
}
