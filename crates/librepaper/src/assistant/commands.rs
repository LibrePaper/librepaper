//! Slash commands typed in the chat. ACP treats a prompt as a command only
//! when its text starts with `/name`, so a message is split at every
//! advertised command and each piece is sent as its own turn.

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Segment {
    /// Ordinary request text, sent wrapped like any other task prompt.
    Text(String),
    /// A recognised command with its argument, sent verbatim.
    Command(String),
}

/// Split `text` at each `/name` that starts the text or follows whitespace and
/// whose name exactly matches an advertised command. A command runs up to the
/// next recognised command, so the text after it is its argument. Anything
/// else beginning with a slash stays plain text.
pub(super) fn split_commands(text: &str, known: &[String]) -> Vec<Segment> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for word in text.split_inclusive(char::is_whitespace) {
        let token = word.trim_end();
        if token
            .strip_prefix('/')
            .is_some_and(|name| known.iter().any(|known| known == name))
        {
            starts.push(offset);
        }
        offset += word.len();
    }
    let Some(&first) = starts.first() else {
        return vec![Segment::Text(text.to_string())];
    };
    let mut segments = Vec::new();
    let lead = text[..first].trim();
    if !lead.is_empty() {
        segments.push(Segment::Text(lead.to_string()));
    }
    for (index, &start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(text.len());
        segments.push(Segment::Command(text[start..end].trim().to_string()));
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<String> {
        vec!["compact".into(), "review".into()]
    }

    fn text(value: &str) -> Segment {
        Segment::Text(value.into())
    }

    fn command(value: &str) -> Segment {
        Segment::Command(value.into())
    }

    #[test]
    fn command_at_the_start_carries_its_argument() {
        assert_eq!(
            split_commands("/review the intro", &known()),
            vec![command("/review the intro")]
        );
    }

    #[test]
    fn command_in_the_middle_splits_the_text() {
        assert_eq!(
            split_commands("tighten this then /compact", &known()),
            vec![text("tighten this then"), command("/compact")]
        );
    }

    #[test]
    fn several_commands_each_take_the_text_up_to_the_next() {
        assert_eq!(
            split_commands("fix it /review section 2 /compact", &known()),
            vec![
                text("fix it"),
                command("/review section 2"),
                command("/compact")
            ]
        );
    }

    #[test]
    fn unknown_command_is_plain_text() {
        assert_eq!(
            split_commands("please /unknown now", &known()),
            vec![text("please /unknown now")]
        );
    }

    #[test]
    fn paths_and_alternatives_are_plain_text() {
        assert_eq!(
            split_commands("see /usr/bin and/or /review-x", &known()),
            vec![text("see /usr/bin and/or /review-x")]
        );
        assert_eq!(
            split_commands("a/compact b", &known()),
            vec![text("a/compact b")]
        );
    }

    #[test]
    fn command_without_arguments_stands_alone() {
        assert_eq!(
            split_commands("/compact", &known()),
            vec![command("/compact")]
        );
    }

    #[test]
    fn blank_text_before_a_command_is_dropped() {
        assert_eq!(
            split_commands("  \n /compact  ", &known()),
            vec![command("/compact")]
        );
    }

    #[test]
    fn no_known_commands_keeps_one_text_segment() {
        assert_eq!(
            split_commands("/compact now", &[]),
            vec![text("/compact now")]
        );
    }

    #[test]
    fn a_command_after_a_newline_is_recognised() {
        assert_eq!(
            split_commands("first line\n/review this", &known()),
            vec![text("first line"), command("/review this")]
        );
    }
}
