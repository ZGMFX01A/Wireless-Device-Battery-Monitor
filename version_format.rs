//! Shared by the build script and its regression tests. A release version is data,
//! never an unchecked fragment of a Windows resource script or a download path.
pub fn normalize(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let version = raw
        .strip_prefix('v')
        .or_else(|| raw.strip_prefix('V'))
        .unwrap_or(raw);
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|p| {
            p.is_empty()
                || !p.bytes().all(|b| b.is_ascii_digit())
                || (p.len() > 1 && p.starts_with('0'))
                || p.parse::<u16>().is_err()
        })
    {
        return Err("Expected a stable MAJOR.MINOR.PATCH version; each component must fit a Windows version field".into());
    }
    Ok(version.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_versions_only() {
        assert_eq!(normalize("v2.8.1").unwrap(), "2.8.1");
        assert_eq!(normalize("0.1.3").unwrap(), "0.1.3");
        for v in [
            "",
            "v",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "65536.0.0",
            "1.2.3-beta",
            "1.2.3+build",
            "1.2.3\"\nBLOCK",
            "$(whoami)",
        ] {
            assert!(normalize(v).is_err(), "{v:?}");
        }
    }
}
