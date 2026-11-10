//! Visit history persisted as JSON.
//!
//! Separate file per concern (product decision): history lives in
//! `history.json`, session in `session.json`, both under the data dir.

use serde::{Deserialize, Serialize};

/// Platform data dir: `~/.local/share/lattice/`, created on demand.
pub fn data_dir() -> std::path::PathBuf {
    let base = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let dir = std::path::PathBuf::from(base)
        .join(".local")
        .join("share")
        .join("lattice");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn data_file(name: &str) -> std::path::PathBuf {
    data_dir().join(name)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Visit {
    pub url: String,
    pub title: Option<String>,
    pub at_unix_secs: u64,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct HistoryLog {
    pub visits: Vec<Visit>,
}

impl HistoryLog {
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn push(&mut self, url: &str) {
        // Collapse immediate repeats (reloads) to keep the log useful.
        if self
            .visits
            .last()
            .map(|v| v.url.as_str() != url)
            .unwrap_or(true)
        {
            self.visits.push(Visit {
                url: url.to_string(),
                title: None,
                at_unix_secs: now_secs(),
            });
            const CAP: usize = 5000;
            if self.visits.len() > CAP {
                let drop = self.visits.len() - CAP;
                self.visits.drain(..drop);
            }
        }
    }

    /// Fill the newest untitled visit for `url` (titles arrive async).
    pub fn retitle(&mut self, url: &str, title: &str) {
        if let Some(v) = self
            .visits
            .iter_mut()
            .rev()
            .find(|v| v.url == url && v.title.is_none())
        {
            v.title = Some(title.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_collapses_reloads() {
        let mut h = HistoryLog::default();
        h.push("https://a.example/");
        h.push("https://a.example/");
        h.push("https://b.example/");
        assert_eq!(h.visits.len(), 2);
    }

    #[test]
    fn retitle_fills_newest_untitled() {
        let mut h = HistoryLog::default();
        h.push("https://a.example/");
        h.push("https://b.example/");
        h.retitle("https://a.example/", "A");
        assert_eq!(h.visits[0].title.as_deref(), Some("A"));
        assert_eq!(h.visits[1].title, None);
    }

    #[test]
    fn round_trip_file() {
        let dir = std::env::temp_dir().join("lattice-history-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("history.json");
        let mut h = HistoryLog::default();
        h.push("https://a.example/");
        h.retitle("https://a.example/", "A");
        h.save(&path);
        let loaded = HistoryLog::load(&path);
        assert_eq!(loaded.visits.len(), 1);
        assert_eq!(loaded.visits[0].title.as_deref(), Some("A"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_is_empty() {
        let h = HistoryLog::load(std::path::Path::new("/nonexistent-lattice-test/history.json"));
        assert!(h.visits.is_empty());
    }
}
