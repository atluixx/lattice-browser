//! Lattice core: tabs, navigation, policy, session (M0 stubs).

pub mod permissions;
pub mod policy;
pub mod session;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabState {
    Loading,
    Live,
    Throttled,
    Closed,
}

#[derive(Debug, Clone)]
pub struct Tab {
    pub id: u64,
    pub url: String,
    pub title: String,
    pub state: TabState,
}

impl Tab {
    pub fn new(id: u64, url: String) -> Self {
        Self {
            id,
            url,
            title: String::new(),
            state: TabState::Loading,
        }
    }
}
