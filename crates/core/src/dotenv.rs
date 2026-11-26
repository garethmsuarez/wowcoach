//! Minimal `.env` reader for API credentials (`BLIZZARD_CLIENT_ID`,
//! `WCL_CLIENT_SECRET`, ...). Supports `KEY=value`, `export KEY=value`,
//! single/double quotes and `#` comments; nothing else. Values already set in
//! the environment win, so the file never overrides an explicit export.

/// Parse `.env` text into (key, value) pairs, in file order.
pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let v = v.trim();
        let v = if v.len() >= 2
            && ((v.starts_with('"') && v.ends_with('"'))
                || (v.starts_with('\'') && v.ends_with('\'')))
        {
            &v[1..v.len() - 1]
        } else {
            // Unquoted: drop a trailing " # comment".
            v.split(" #").next().unwrap_or("").trim_end()
        };
        out.push((k.to_string(), v.to_string()));
    }
    out
}

/// Entries from `path` whose keys aren't set in the environment yet.
/// A missing file is not an error.
pub fn missing_from_env(path: &std::path::Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    parse(&text)
        .into_iter()
        .filter(|(k, _)| std::env::var_os(k).is_none())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_common_forms() {
        let got = parse(
            "# creds\n\
             BLIZZARD_CLIENT_ID=abc123\n\
             export WCL_CLIENT_ID = \"with spaces\"\n\
             WCL_CLIENT_SECRET='single'\n\
             EMPTY=\n\
             TRAILING=value # comment\n\
             HASH=\"a#b\"\n\
             not a pair\n\
             BAD KEY=x\n",
        );
        let want = [
            ("BLIZZARD_CLIENT_ID", "abc123"),
            ("WCL_CLIENT_ID", "with spaces"),
            ("WCL_CLIENT_SECRET", "single"),
            ("EMPTY", ""),
            ("TRAILING", "value"),
            ("HASH", "a#b"),
        ];
        let got: Vec<(&str, &str)> = got.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn missing_file_is_fine() {
        assert!(super::missing_from_env(std::path::Path::new("/nonexistent/.env")).is_empty());
    }
}
