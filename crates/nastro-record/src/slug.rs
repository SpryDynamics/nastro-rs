//! Recording-name slugification, ported from the Go `internal/record` package's
//! `Slugify`: ASCII-fold common accented characters, lowercase, collapse runs of
//! non-alphanumerics to a single dash, trim leading/trailing dashes.

/// Fold a single character to its closest plain-ASCII equivalent for the common
/// Latin accented letters. Anything not covered passes through unchanged (it will
/// just be treated as a non-alphanumeric separator by [`slugify`] unless it already
/// is ASCII alphanumeric).
fn fold_char(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' | 'ā' => 'a',
        'À' | 'Á' | 'Â' | 'Ä' | 'Ã' | 'Å' | 'Ā' => 'A',
        'è' | 'é' | 'ê' | 'ë' | 'ē' => 'e',
        'È' | 'É' | 'Ê' | 'Ë' | 'Ē' => 'E',
        'ì' | 'í' | 'î' | 'ï' | 'ī' => 'i',
        'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' => 'I',
        'ò' | 'ó' | 'ô' | 'ö' | 'õ' | 'ō' => 'o',
        'Ò' | 'Ó' | 'Ô' | 'Ö' | 'Õ' | 'Ō' => 'O',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' => 'u',
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' => 'U',
        'ñ' => 'n',
        'Ñ' => 'N',
        'ç' => 'c',
        'Ç' => 'C',
        'ß' => 's',
        other => other,
    }
}

/// Turn free-form recording-name input into a filesystem-safe slug suitable for
/// appending to a `YYYY-MM-DD-HHMM` directory name.
///
/// Empty input (or input that folds to nothing alphanumeric, e.g. all punctuation)
/// yields an empty string - callers should treat that the same as "no slug" (see
/// [`crate::session`] / `nastro_records::build_dir_name`'s `None` case).
pub fn slugify(input: &str) -> String {
    let mut slug = String::with_capacity(input.len());
    let mut pending_dash = false;

    for raw in input.chars() {
        let c = fold_char(raw);
        if c.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(c.to_ascii_lowercase());
            pending_dash = false;
        } else {
            pending_dash = true;
        }
    }

    slug
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_plain_ascii() {
        assert_eq!(slugify("Weekly Standup"), "weekly-standup");
    }

    #[test]
    fn slugify_collapses_runs_of_punctuation_and_spaces() {
        assert_eq!(
            slugify("Weekly   Standup -- Notes!!"),
            "weekly-standup-notes"
        );
    }

    #[test]
    fn slugify_folds_accented_characters() {
        assert_eq!(slugify("Café Münchën"), "cafe-munchen");
    }

    #[test]
    fn slugify_trims_leading_and_trailing_separators() {
        assert_eq!(slugify("  -- Weekly Sync -- "), "weekly-sync");
    }

    #[test]
    fn slugify_empty_input_is_empty() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn slugify_all_punctuation_is_empty() {
        assert_eq!(slugify("!!! --- ???"), "");
    }

    #[test]
    fn slugify_preserves_digits() {
        assert_eq!(slugify("1:1 with Alex"), "1-1-with-alex");
    }
}
