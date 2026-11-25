//! Launcher self-update check against GitHub Releases.

use std::time::Duration;

use crate::config::{REPOSITORY, VERSION};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate,
    Available { version: String, url: String },
    NoReleases,
}

/// Parses `v1.2.3`, `1.2`, `1.2.3-beta` into comparable numeric parts.
pub fn parse_version(s: &str) -> Option<Vec<u64>> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let parts: Option<Vec<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(mut a), Some(mut b)) => {
            let len = a.len().max(b.len());
            a.resize(len, 0);
            b.resize(len, 0);
            a > b
        }
        _ => false,
    }
}

pub fn interpret_release(json: &str, current: &str) -> Result<UpdateStatus, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("invalid response: {e}"))?;
    let tag = value.get("tag_name").and_then(|v| v.as_str()).ok_or("release has no tag")?;
    let url = value
        .get("html_url")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("https://github.com/{REPOSITORY}/releases"));
    Ok(if is_newer(tag, current) {
        UpdateStatus::Available { version: tag.trim_start_matches('v').to_owned(), url }
    } else {
        UpdateStatus::UpToDate
    })
}

/// Blocking; call from a worker thread.
pub fn check() -> Result<UpdateStatus, String> {
    let client = crate::installer::http_client()?;
    let response = client
        .get(format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(Duration::from_secs(20))
        .send()
        .map_err(|e| format!("could not reach GitHub: {e}"))?;
    match response.status().as_u16() {
        404 => Ok(UpdateStatus::NoReleases),
        200 => interpret_release(&response.text().map_err(|e| e.to_string())?, VERSION),
        code => Err(format!("GitHub answered HTTP {code}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1"));
        assert!(is_newer("1.0.0-rc1", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
        assert_eq!(parse_version("v1.2.3+build"), Some(vec![1, 2, 3]));
    }

    #[test]
    fn interprets_release_json() {
        let json = r#"{"tag_name":"v0.3.0","html_url":"https://github.com/oriilol/cutmary/releases/tag/v0.3.0"}"#;
        assert_eq!(
            interpret_release(json, "0.1.0").unwrap(),
            UpdateStatus::Available {
                version: "0.3.0".into(),
                url: "https://github.com/oriilol/cutmary/releases/tag/v0.3.0".into()
            }
        );
        assert_eq!(interpret_release(json, "0.3.0").unwrap(), UpdateStatus::UpToDate);
        assert!(interpret_release("{}", "0.1.0").is_err());
    }
}
