use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieRule {
    Allow,
    Block,
}

#[derive(Debug, Clone)]
pub struct PolicyEngine {
    per_site: HashMap<String, CookieRule>,
    block_third_party: bool,
    blocklist: Vec<String>,
}

impl PolicyEngine {
    pub fn new(block_third_party: bool) -> Self {
        Self {
            per_site: HashMap::new(),
            block_third_party,
            blocklist: Vec::new(),
        }
    }

    pub fn set_site_rule(&mut self, domain: &str, rule: CookieRule) {
        self.per_site.insert(domain.to_lowercase(), rule);
    }

    pub fn load_blocklist(&mut self, entries: Vec<String>) {
        self.blocklist = entries;
    }

    fn domain_of(url: &str) -> String {
        url.split("://")
            .nth(1)
            .unwrap_or(url)
            .split('/')
            .next()
            .unwrap_or(url)
            .split(':')
            .next()
            .unwrap_or(url)
            .to_lowercase()
    }

    pub fn allow_cookies(&self, url: &str, is_third_party: bool) -> bool {
        let domain = Self::domain_of(url);
        if let Some(rule) = self.per_site.get(&domain) {
            return *rule == CookieRule::Allow;
        }
        if is_third_party && self.block_third_party {
            return false;
        }
        true
    }

    pub fn allow_request(&self, url: &str) -> bool {
        let lower = url.to_lowercase();
        !self.blocklist.iter().any(|e| lower.contains(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn third_party_blocked_by_default() {
        let engine = PolicyEngine::new(true);
        assert!(!engine.allow_cookies("https://tracker.example/pixel", true));
        assert!(engine.allow_cookies("https://first.example/", false));
    }

    #[test]
    fn per_site_override_wins() {
        let mut engine = PolicyEngine::new(true);
        engine.set_site_rule("tracker.example", CookieRule::Allow);
        assert!(engine.allow_cookies("https://tracker.example/pixel", true));
        engine.set_site_rule("tracker.example", CookieRule::Block);
        assert!(!engine.allow_cookies("https://tracker.example/", false));
    }

    #[test]
    fn blocklist_blocks_requests() {
        let mut engine = PolicyEngine::new(true);
        engine.load_blocklist(vec!["ads.example".to_string()]);
        assert!(!engine.allow_request("https://ads.example/banner.js"));
        assert!(engine.allow_request("https://content.example/app.js"));
    }
}
