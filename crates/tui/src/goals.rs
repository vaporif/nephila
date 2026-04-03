use nephila_core::id::{AgentId, ObjectiveId};
use nephila_core::objective::ObjectiveStatus;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct GoalObjective {
    pub id: Option<ObjectiveId>,
    pub file_path: PathBuf,
    pub title: String,
    pub content: String,
    pub status: ObjectiveStatus,
    pub agent_id: Option<AgentId>,
    pub children: Vec<GoalSubObjective>,
}

#[derive(Debug, Clone)]
pub struct GoalSubObjective {
    pub id: ObjectiveId,
    pub description: String,
    pub status: ObjectiveStatus,
    pub children: Vec<GoalSubObjective>,
}

#[derive(Debug, Clone)]
pub enum ObjectiveItem {
    Root(GoalObjective),
    Sub(GoalSubObjective),
}

impl ObjectiveItem {
    pub fn status(&self) -> ObjectiveStatus {
        match self {
            ObjectiveItem::Root(g) => g.status,
            ObjectiveItem::Sub(s) => s.status,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            ObjectiveItem::Root(g) => &g.title,
            ObjectiveItem::Sub(s) => &s.description,
        }
    }

    pub fn agent_id(&self) -> Option<AgentId> {
        match self {
            ObjectiveItem::Root(g) => g.agent_id,
            ObjectiveItem::Sub(_) => None,
        }
    }

    pub fn objective_id(&self) -> Option<ObjectiveId> {
        match self {
            ObjectiveItem::Root(g) => g.id,
            ObjectiveItem::Sub(s) => Some(s.id),
        }
    }
}

impl GoalObjective {
    pub fn from_file(path: &Path) -> io::Result<Self> {
        let content = fs::read_to_string(path)?;
        let title = extract_title(&content).unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("untitled")
                .to_string()
        });

        Ok(Self {
            id: None,
            file_path: path.to_path_buf(),
            title,
            content,
            status: ObjectiveStatus::Pending,
            agent_id: None,
            children: Vec::new(),
        })
    }
}

fn extract_title(content: &str) -> Option<String> {
    content
        .lines()
        .filter_map(|line| line.trim().strip_prefix("# "))
        .map(str::trim)
        .find(|t| !t.is_empty())
        .map(str::to_string)
}

pub fn scan_goals_dir(goals_dir: &Path) -> io::Result<Vec<GoalObjective>> {
    if !goals_dir.exists() {
        return Ok(Vec::new());
    }

    let mut goals = Vec::new();
    for entry in fs::read_dir(goals_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            match GoalObjective::from_file(&path) {
                Ok(goal) => goals.push(goal),
                Err(e) => {
                    tracing::warn!("Failed to read goal file {:?}: {}", path, e);
                }
            }
        }
    }
    goals.sort_by(|a, b| a.file_path.cmp(&b.file_path));
    Ok(goals)
}

pub fn create_template_file(goals_dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(goals_dir)?;
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{timestamp}-objective.md");
    let path = goals_dir.join(filename);

    fs::write(
        &path,
        "# New Objective\n\nDescribe your objective here. Claude will interpret this freely.\n",
    )?;
    Ok(path)
}

const MAPPING_FILE: &str = ".nephila-goals.json";

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct GoalMapping {
    pub(crate) entries: HashMap<String, ObjectiveId>,
}

pub(crate) fn load_mapping(goals_dir: &Path) -> GoalMapping {
    let path = goals_dir.join(MAPPING_FILE);
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            tracing::warn!("Invalid goals mapping file: {e}");
            GoalMapping::default()
        }),
        Err(_) => GoalMapping::default(),
    }
}

pub(crate) fn save_mapping(goals_dir: &Path, mapping: &GoalMapping) -> io::Result<()> {
    let path = goals_dir.join(MAPPING_FILE);
    let json = serde_json::to_string_pretty(mapping).map_err(io::Error::other)?;
    fs::write(path, json)
}

pub(crate) fn reconcile_with_mapping(goals_dir: &Path, goals: &mut [GoalObjective]) {
    let mut mapping = load_mapping(goals_dir);
    let mut changed = false;

    for goal in goals.iter_mut() {
        if let Some(filename) = goal.file_path.file_name().and_then(|f| f.to_str()) {
            if let Some(id) = mapping.entries.get(filename) {
                goal.id = Some(*id);
            } else {
                let id = ObjectiveId::new();
                goal.id = Some(id);
                mapping.entries.insert(filename.to_string(), id);
                changed = true;
            }
        }
    }

    if changed && let Err(e) = save_mapping(goals_dir, &mapping) {
        tracing::warn!("Failed to save goals mapping: {e}");
    }
}

pub fn open_in_editor(path: &Path) -> io::Result<bool> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
    let mut parts = editor.split_whitespace();
    let bin = parts.next().unwrap_or("vi");
    let status = std::process::Command::new(bin)
        .args(parts)
        .arg(path)
        .status()?;
    Ok(status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn goal_objective_from_file_uses_heading_as_title() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("my-feature.md");
        fs::write(&path, "# Refactor Auth\n\nDetails here.").unwrap();

        let goal = GoalObjective::from_file(&path).unwrap();
        assert_eq!(goal.title, "Refactor Auth");
        assert!(goal.content.contains("Details here."));
        assert_eq!(goal.status, ObjectiveStatus::Pending);
        assert!(goal.id.is_none());
        assert!(goal.agent_id.is_none());
        assert!(goal.children.is_empty());
    }

    #[test]
    fn goal_objective_from_file_falls_back_to_filename() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("fix-ci.md");
        fs::write(&path, "No heading, just text.").unwrap();

        let goal = GoalObjective::from_file(&path).unwrap();
        assert_eq!(goal.title, "fix-ci");
    }

    #[test]
    fn scan_goals_dir_finds_md_files() {
        let dir = TempDir::new().unwrap();
        let goals_dir = dir.path().join("goals");
        fs::create_dir(&goals_dir).unwrap();
        fs::write(goals_dir.join("a.md"), "# Alpha").unwrap();
        fs::write(goals_dir.join("b.md"), "# Beta").unwrap();
        fs::write(goals_dir.join("c.txt"), "not a goal").unwrap();

        let goals = scan_goals_dir(&goals_dir).unwrap();
        assert_eq!(goals.len(), 2);
        let titles: Vec<&str> = goals.iter().map(|g| g.title.as_str()).collect();
        assert!(titles.contains(&"Alpha"));
        assert!(titles.contains(&"Beta"));
    }

    #[test]
    fn scan_goals_dir_returns_empty_for_missing_dir() {
        let goals = scan_goals_dir(Path::new("/nonexistent/goals"));
        assert!(goals.is_ok());
        assert!(goals.unwrap().is_empty());
    }

    #[test]
    fn save_and_load_mapping() {
        let dir = TempDir::new().unwrap();
        let goals_dir = dir.path().join("goals");
        fs::create_dir(&goals_dir).unwrap();

        let mut map = GoalMapping::default();
        let id = ObjectiveId::new();
        map.entries.insert("feature-x.md".into(), id);

        save_mapping(&goals_dir, &map).unwrap();
        let loaded = load_mapping(&goals_dir);
        assert_eq!(loaded.entries.get("feature-x.md"), Some(&id));
    }

    #[test]
    fn load_mapping_returns_default_for_missing_file() {
        let dir = TempDir::new().unwrap();
        let mapping = load_mapping(dir.path());
        assert!(mapping.entries.is_empty());
    }

    #[test]
    fn reconcile_assigns_ids_from_mapping() {
        let dir = TempDir::new().unwrap();
        let goals_dir = dir.path().join("goals");
        fs::create_dir(&goals_dir).unwrap();
        fs::write(goals_dir.join("task-a.md"), "# Task A").unwrap();

        let id = ObjectiveId::new();
        let mut map = GoalMapping::default();
        map.entries.insert("task-a.md".into(), id);
        save_mapping(&goals_dir, &map).unwrap();

        let mut goals = scan_goals_dir(&goals_dir).unwrap();
        reconcile_with_mapping(&goals_dir, &mut goals);

        assert_eq!(goals[0].id, Some(id));
    }

    #[test]
    fn create_template_file_writes_and_returns_path() {
        let dir = TempDir::new().unwrap();
        let goals_dir = dir.path().join("goals");

        let path = create_template_file(&goals_dir).unwrap();
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# New Objective"));
        assert!(content.contains("Describe your objective here"));
    }
}
