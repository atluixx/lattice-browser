//! Per-site permissions (MVP: default-deny for powerful features).

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    Camera,
    Microphone,
    Notifications,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    Prompt,
}

impl Default for Decision {
    fn default() -> Self {
        Decision::Deny
    }
}

#[derive(Debug, Default)]
pub struct PermissionStore {
    rules: HashMap<(String, Permission), Decision>,
}

impl PermissionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, domain: &str, permission: Permission, decision: Decision) {
        self.rules
            .insert((domain.to_lowercase(), permission), decision);
    }

    /// MVP: everything defaults to Deny unless explicitly allowed.
    /// `Prompt` resolves to Deny in MVP (no prompt UI yet).
    pub fn check(&self, domain: &str, permission: Permission) -> Decision {
        match self.rules.get(&(domain.to_lowercase(), permission)) {
            Some(Decision::Prompt) | None => Decision::Deny,
            Some(d) => *d,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_deny() {
        let store = PermissionStore::new();
        assert_eq!(
            store.check("example.com", Permission::Camera),
            Decision::Deny
        );
        assert_eq!(
            store.check("example.com", Permission::Notifications),
            Decision::Deny
        );
    }

    #[test]
    fn explicit_allow_and_prompt_fallback() {
        let mut store = PermissionStore::new();
        store.set("meet.example", Permission::Camera, Decision::Allow);
        store.set("news.example", Permission::Notifications, Decision::Prompt);
        assert_eq!(
            store.check("meet.example", Permission::Camera),
            Decision::Allow
        );
        // No prompt UI in MVP: Prompt resolves to Deny.
        assert_eq!(
            store.check("news.example", Permission::Notifications),
            Decision::Deny
        );
        // Other permissions on the same domain stay denied.
        assert_eq!(
            store.check("meet.example", Permission::Microphone),
            Decision::Deny
        );
    }
}
