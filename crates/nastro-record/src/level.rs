//! Parses the `nastro-tap` stdout level-line contract: `level sys=0.42 mic=0.10`,
//! with either field potentially omitted if that source isn't active (e.g.
//! `--mic-only` mode only ever emits `mic=`).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelReading {
    pub sys: Option<f32>,
    pub mic: Option<f32>,
}

/// Parse a single stdout line from `nastro-tap`. Returns `None` for anything that
/// isn't a recognizable `level ...` line (diagnostic lines, blank lines, etc. are
/// simply not level readings - not an error).
pub fn parse_level_line(line: &str) -> Option<LevelReading> {
    let rest = line.trim().strip_prefix("level ")?;

    let mut sys = None;
    let mut mic = None;

    for token in rest.split_whitespace() {
        if let Some(value) = token.strip_prefix("sys=") {
            sys = value.parse::<f32>().ok();
        } else if let Some(value) = token.strip_prefix("mic=") {
            mic = value.parse::<f32>().ok();
        }
    }

    if sys.is_none() && mic.is_none() {
        return None;
    }

    Some(LevelReading { sys, mic })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_fields() {
        let reading = parse_level_line("level sys=0.42 mic=0.10").unwrap();
        assert_eq!(reading.sys, Some(0.42));
        assert_eq!(reading.mic, Some(0.10));
    }

    #[test]
    fn parses_sys_only() {
        let reading = parse_level_line("level sys=0.75").unwrap();
        assert_eq!(reading.sys, Some(0.75));
        assert_eq!(reading.mic, None);
    }

    #[test]
    fn parses_mic_only() {
        let reading = parse_level_line("level mic=0.05").unwrap();
        assert_eq!(reading.sys, None);
        assert_eq!(reading.mic, Some(0.05));
    }

    #[test]
    fn tolerates_trailing_whitespace_and_newline() {
        let reading = parse_level_line("level sys=0.1 mic=0.2\r\n").unwrap();
        assert_eq!(reading.sys, Some(0.1));
        assert_eq!(reading.mic, Some(0.2));
    }

    #[test]
    fn ignores_non_level_lines() {
        assert!(parse_level_line("Info: starting capture").is_none());
        assert!(parse_level_line("").is_none());
        assert!(parse_level_line("levelsys=0.1").is_none());
    }

    #[test]
    fn malformed_values_are_treated_as_absent() {
        let reading = parse_level_line("level sys=notanumber mic=0.2").unwrap();
        assert_eq!(reading.sys, None);
        assert_eq!(reading.mic, Some(0.2));
    }
}
