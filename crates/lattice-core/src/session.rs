use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionTab {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub tabs: Vec<SessionTab>,
    pub active: usize,
}

impl Session {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = Session {
            tabs: vec![SessionTab {
                url: "about:blank".to_string(),
                title: String::new(),
            }],
            active: 0,
        };
        let json = s.to_json();
        assert_eq!(Session::from_json(&json), Some(s));
    }

    #[test]
    fn bad_json_returns_none() {
        assert_eq!(Session::from_json("not json"), None);
    }
}
