use once_cell::sync::Lazy;

pub const USER_AGENT: &str = concat!(
    "kuribottigs/custom-launcher/",
    env!("CARGO_PKG_VERSION"),
    " (oxide-launcher)"
);

static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .expect("failed to build HTTP client")
});

/// Shared HTTP client. Modrinth requires a meaningful User-Agent, so all
/// requests go through this client.
pub fn client() -> reqwest::Client {
    CLIENT.clone()
}
