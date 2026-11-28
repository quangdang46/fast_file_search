//! Background update checker — compares the embedded crate version against
//! the latest stable GitHub release tag to surface upgrade notices.

use std::sync::OnceLock;

const REPO: &str = "dmtrKovalenko/fff.nvim";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Holds the result of the update check (empty string = up to date or check failed).
static UPDATE_NOTICE: OnceLock<String> = OnceLock::new();

/// Returns the update notice if the check has completed, empty string otherwise.
pub fn get_update_notice() -> &'static str {
    UPDATE_NOTICE.get().map(|s| s.as_str()).unwrap_or("")
}

/// Kick off the update check in a background thread so it never blocks the server.
pub fn spawn_update_check() {
    std::thread::spawn(|| {
        let notice = check_latest_release();
        let _ = UPDATE_NOTICE.set(notice);
    });
}

fn check_latest_release() -> String {
    match fetch_latest_stable_tag() {
        Ok(tag) => compare_versions(CURRENT_VERSION, &tag),
        Err(_) => String::new(),
    }
}

/// Parse `major.minor.patch`-style versions into a comparable tuple.
/// Pre-release suffixes are ignored for ordering (`0.10.2-nightly.x` sorts
/// as `0.10.2`); returns `None` when no numeric triple can be extracted.
fn version_tuple(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches('v');
    let mut parts = v.split('.');
    let mut nums = [0u64; 3];
    for slot in &mut nums {
        let p = parts.next()?;
        let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        *slot = digits.parse().ok()?;
    }
    Some((nums[0], nums[1], nums[2]))
}

/// Compare the crate version against a release tag.
/// Returns an update notice string, or empty if up-to-date.
fn compare_versions(current_version: &str, release_tag: &str) -> String {
    let tag = release_tag.trim();
    let tag_version = tag.strip_prefix('v').unwrap_or(tag);
    if tag.is_empty() || tag_version == current_version {
        return String::new();
    }

    // Only claim an update when the tag is strictly newer — plain string
    // inequality used to advertise "updates" pointing at older releases.
    // Unparsable versions fall back to the old not-equal behaviour.
    if let (Some(cur), Some(new)) = (version_tuple(current_version), version_tuple(tag_version))
        && cur >= new
    {
        return String::new();
    }

    format!(
        "\n[ffs update available ({current_version} -> {tag_version}): `curl -fsSL https://raw.githubusercontent.com/{REPO}/main/install-mcp.sh | bash`]\n"
    )
}

// Uses /releases/latest — GitHub excludes prereleases here, matching the
// stable channel that install-mcp.sh installs from.
fn fetch_latest_stable_tag() -> Result<String, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "5",
            "-H",
            "Accept: application/vnd.github.v3+json",
            &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        ])
        .output()?;

    if !output.status.success() {
        return Err("curl failed".into());
    }

    let body = String::from_utf8(output.stdout)?;
    let release: serde_json::Value = serde_json::from_str(&body)?;
    let tag = release
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(tag)
}

#[cfg(test)]
mod tests {
    use super::compare_versions;

    #[test]
    fn same_version_with_v_prefix_is_silent() {
        assert_eq!(compare_versions("0.10.1", "v0.10.1"), "");
    }

    #[test]
    fn same_version_without_v_prefix_is_silent() {
        assert_eq!(compare_versions("0.10.1", "0.10.1"), "");
    }

    #[test]
    fn empty_tag_is_silent() {
        assert_eq!(compare_versions("0.10.1", ""), "");
        assert_eq!(compare_versions("0.10.1", "   "), "");
    }

    #[test]
    fn older_current_reports_update() {
        let notice = compare_versions("0.10.0", "v0.10.1");
        assert!(notice.contains("0.10.0 -> 0.10.1"), "got: {notice}");
        assert!(notice.contains("install-mcp.sh"));
    }

    #[test]
    fn nightly_tag_never_equals_stable_current() {
        let notice = compare_versions("0.10.1", "0.10.2-nightly.6a239e9");
        assert!(!notice.is_empty());
        assert!(notice.contains("0.10.1 -> 0.10.2-nightly.6a239e9"));
    }

    #[test]
    fn older_release_tag_is_silent() {
        // Regression: string inequality alone claimed an "update" when the
        // installed version was actually newer than the latest tag.
        assert_eq!(compare_versions("0.10.2", "v0.10.1"), "");
        assert_eq!(compare_versions("0.11.0", "v0.10.9"), "");
        assert_eq!(compare_versions("1.0.0", "v0.99.99"), "");
    }

    #[test]
    fn newer_release_tag_still_notifies() {
        assert!(!compare_versions("0.10.1", "v0.10.2").is_empty());
        assert!(!compare_versions("0.9.9", "v0.10.0").is_empty());
    }

    #[test]
    fn unparsable_versions_fall_back_to_inequality() {
        // Weird tags keep the old behaviour rather than crashing the check.
        let notice = compare_versions("0.10.1", "nightly-build");
        assert!(notice.contains("0.10.1 -> nightly-build"));
    }
}
