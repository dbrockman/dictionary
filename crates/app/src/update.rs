//! The app's version, and asking GitHub for the latest release.
//!
//! Nothing is fetched unless the user asks, so startup never waits on the
//! network.

use std::time::Duration;

use serde::Deserialize;

/// The version this build was made from.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The project's home on GitHub.
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

const TIMEOUT: Duration = Duration::from_secs(10);

/// A published release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// The version, without the tag's leading `v`.
    pub version: String,
    /// The release's page on GitHub.
    pub url: String,
}

impl Release {
    /// Whether this release is newer than the running build.
    pub fn is_newer(&self) -> bool {
        is_newer(&self.version, VERSION)
    }
}

#[derive(Deserialize)]
struct ReleaseJson {
    tag_name: String,
    html_url: String,
}

/// Fetches the latest release. `Ok(None)` means nothing is published yet.
/// This blocks, so run it on a background thread.
pub fn latest_release() -> Result<Option<Release>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        // A 404 (no releases) is an answer, not an error.
        .http_status_as_error(false)
        .build()
        .into();
    let repo = REPOSITORY.trim_start_matches("https://github.com/");
    let mut response = agent
        .get(format!(
            "https://api.github.com/repos/{repo}/releases/latest"
        ))
        // GitHub rejects API requests without a user agent.
        .header(
            "User-Agent",
            concat!("dictionary/", env!("CARGO_PKG_VERSION")),
        )
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("Couldn’t reach GitHub: {e}"))?;
    match response.status().as_u16() {
        200 => {}
        404 => return Ok(None),
        status => return Err(format!("GitHub answered with HTTP {status}.")),
    }
    let release: ReleaseJson = response
        .body_mut()
        .read_json()
        .map_err(|e| format!("Couldn’t read GitHub’s answer: {e}"))?;
    Ok(Some(Release {
        version: release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name)
            .to_owned(),
        url: release.html_url,
    }))
}

/// Compares dotted version numbers, e.g. `0.10.0` > `0.9.2`. A pre-release
/// or build suffix (`-rc.1`, `+abc`) is ignored, and missing parts count as 0.
fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(version: &str) -> Vec<u64> {
        let core = version.split(['-', '+']).next().unwrap_or("");
        let mut parts: Vec<u64> = core.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        while parts.last() == Some(&0) {
            parts.pop();
        }
        parts
    }
    parts(candidate) > parts(current)
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.2"));
        assert!(is_newer("1.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.1.0-rc.1", "0.1.0"));
    }
}
