pub fn explain(pattern: &str) -> Vec<String> {
    if pattern.is_empty() {
        return vec!["Enter a regex to see an explanation.".to_string()];
    }

    let mut parser = Parser::new(pattern);
    let mut out = Vec::new();
    parser.explain_sequence(&mut out, None, 0);
    out.truncate(80);
    out
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(pattern: &str) -> Self {
        Self {
            chars: pattern.chars().collect(),
            pos: 0,
        }
    }

    fn explain_sequence(&mut self, out: &mut Vec<String>, until: Option<char>, depth: usize) {
        while let Some(ch) = self.peek() {
            if Some(ch) == until {
                self.pos += 1;
                return;
            }

            if ch == '|' {
                self.pos += 1;
                push(out, depth, "| starts an alternative branch");
                continue;
            }

            let Some(atom) = self.parse_atom(depth) else {
                self.pos += 1;
                continue;
            };
            let quantifier = self.parse_quantifier();
            push(out, depth, &apply_quantifier(atom, quantifier));
        }
    }

    fn parse_atom(&mut self, depth: usize) -> Option<String> {
        match self.peek()? {
            '^' => {
                self.pos += 1;
                Some("^ asserts position at the start of a line or string".to_string())
            }
            '$' => {
                self.pos += 1;
                Some("$ asserts position at the end of a line or string".to_string())
            }
            '.' => {
                self.pos += 1;
                Some(". matches any character except newline unless s is enabled".to_string())
            }
            '[' => Some(self.parse_class()),
            '(' => Some(self.parse_group(depth)),
            ')' => {
                self.pos += 1;
                Some(") closes the current group".to_string())
            }
            '\\' => Some(self.parse_escape()),
            '*' | '+' | '?' | '{' => {
                let ch = self.next()?;
                Some(format!("{ch} is a quantifier without a previous token"))
            }
            _ => Some(self.parse_literal()),
        }
    }

    fn parse_literal(&mut self) -> String {
        let mut literal = String::new();
        while let Some(ch) = self.peek() {
            if is_metachar(ch) {
                break;
            }

            literal.push(ch);
            self.pos += 1;

            if self.next_is_quantifier() {
                break;
            }
        }

        format!("{literal} matches the characters literally")
    }

    fn parse_class(&mut self) -> String {
        let start = self.pos;
        self.pos += 1;

        let mut escaped = false;
        while let Some(ch) = self.next() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == ']' {
                break;
            }
        }

        let class = self.slice(start, self.pos);
        if class.starts_with("[^") {
            format!("{class} matches one character not present in the set")
        } else {
            format!("{class} matches one character present in the set")
        }
    }

    fn parse_group(&mut self, depth: usize) -> String {
        self.pos += 1;

        let (label, body_depth) = if self.consume("?P<") {
            let name_start = self.pos;
            while let Some(ch) = self.peek() {
                if ch == '>' {
                    break;
                }
                self.pos += 1;
            }
            let name = self.slice(name_start, self.pos);
            if self.peek() == Some('>') {
                self.pos += 1;
            }
            (
                format!("(?P<{name}>...) captures a match into the named group \"{name}\""),
                depth + 1,
            )
        } else if self.consume("?:") {
            (
                "(?:...) groups tokens without creating a capture group".to_string(),
                depth + 1,
            )
        } else if self.consume("?=") {
            (
                "(?=...) positive lookahead assertion".to_string(),
                depth + 1,
            )
        } else if self.consume("?!") {
            (
                "(?!...) negative lookahead assertion".to_string(),
                depth + 1,
            )
        } else if self.consume("?<=") {
            (
                "(?<=...) positive lookbehind assertion".to_string(),
                depth + 1,
            )
        } else if self.consume("?<!") {
            (
                "(?<!...) negative lookbehind assertion".to_string(),
                depth + 1,
            )
        } else if self.peek() == Some('?') {
            let start = self.pos;
            while let Some(ch) = self.peek() {
                self.pos += 1;
                if ch == ':' {
                    break;
                }
                if ch == ')' {
                    return format!(
                        "({}...) sets regex flags for the surrounding expression",
                        self.slice(start, self.pos)
                    );
                }
            }
            (
                format!(
                    "({}...) sets regex flags for this group",
                    self.slice(start, self.pos)
                ),
                depth + 1,
            )
        } else {
            (
                "(...) captures the matched text into a numbered group".to_string(),
                depth + 1,
            )
        };

        let mut nested = Vec::new();
        self.explain_sequence(&mut nested, Some(')'), body_depth);

        if nested.is_empty() {
            label
        } else {
            let mut lines = Vec::with_capacity(nested.len() + 1);
            lines.push(label);
            lines.extend(nested);
            lines.join("\n")
        }
    }

    fn parse_escape(&mut self) -> String {
        self.pos += 1;
        let Some(ch) = self.next() else {
            return r"\ at the end of the pattern matches a literal backslash".to_string();
        };

        match ch {
            'd' => r"\d matches any digit, equivalent to [0-9]".to_string(),
            'D' => r"\D matches any non-digit character".to_string(),
            'w' => r"\w matches any word character".to_string(),
            'W' => r"\W matches any non-word character".to_string(),
            's' => r"\s matches any whitespace character".to_string(),
            'S' => r"\S matches any non-whitespace character".to_string(),
            'b' => r"\b asserts a word boundary".to_string(),
            'B' => r"\B asserts that the position is not a word boundary".to_string(),
            'A' => r"\A asserts the start of the string".to_string(),
            'z' => r"\z asserts the end of the string".to_string(),
            'n' => r"\n matches a newline character".to_string(),
            'r' => r"\r matches a carriage return character".to_string(),
            't' => r"\t matches a tab character".to_string(),
            other => {
                format!(
                    "\\{other} matches the character \"{other}\" literally or uses a regex escape"
                )
            }
        }
    }

    fn parse_quantifier(&mut self) -> Option<String> {
        let quantifier = match self.peek()? {
            '*' => {
                self.pos += 1;
                Some("zero or more times".to_string())
            }
            '+' => {
                self.pos += 1;
                Some("one or more times".to_string())
            }
            '?' => {
                self.pos += 1;
                Some("zero or one time".to_string())
            }
            '{' => Some(self.parse_count_quantifier()),
            _ => None,
        }?;

        if self.peek() == Some('?') {
            self.pos += 1;
            Some(format!("{quantifier}, as few times as possible"))
        } else {
            Some(quantifier)
        }
    }

    fn parse_count_quantifier(&mut self) -> String {
        let start = self.pos;
        self.pos += 1;
        while let Some(ch) = self.next() {
            if ch == '}' {
                break;
            }
        }

        let quantifier = self.slice(start, self.pos);
        match quantifier
            .trim_start_matches('{')
            .trim_end_matches('}')
            .split_once(',')
        {
            Some((min, "")) => format!("at least {min} times"),
            Some(("", max)) => format!("between zero and {max} times"),
            Some((min, max)) => format!("between {min} and {max} times"),
            None => format!("exactly {} times", quantifier.trim_matches(['{', '}'])),
        }
    }

    fn consume(&mut self, needle: &str) -> bool {
        if !self.starts_with(needle) {
            return false;
        }

        self.pos += needle.chars().count();
        true
    }

    fn starts_with(&self, needle: &str) -> bool {
        needle
            .chars()
            .enumerate()
            .all(|(offset, ch)| self.chars.get(self.pos + offset) == Some(&ch))
    }

    fn next_is_quantifier(&self) -> bool {
        matches!(self.peek(), Some('*' | '+' | '?' | '{'))
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn slice(&self, start: usize, end: usize) -> String {
        self.chars[start..end].iter().collect()
    }
}

fn apply_quantifier(atom: String, quantifier: Option<String>) -> String {
    match quantifier {
        Some(quantifier) => format!("{atom}; repeat the previous token {quantifier}"),
        None => atom,
    }
}

fn push(out: &mut Vec<String>, depth: usize, text: &str) {
    for line in text.lines() {
        let indent = "  ".repeat(depth);
        out.push(format!("{indent}{line}"));
    }
}

fn is_metachar(ch: char) -> bool {
    matches!(
        ch,
        '^' | '$' | '.' | '*' | '+' | '?' | '|' | '[' | ']' | '(' | ')' | '{' | '}' | '\\'
    )
}

#[cfg(test)]
mod tests {
    use super::explain;

    #[test]
    fn keeps_literal_words_together() {
        assert_eq!(
            explain("name"),
            vec!["name matches the characters literally"]
        );
    }

    #[test]
    fn does_not_escape_quotes_in_literal_display() {
        assert_eq!(
            explain(r#""name""#),
            vec![r#""name" matches the characters literally"#]
        );
    }

    #[test]
    fn consumes_named_group_prefix_as_one_construct() {
        let lines = explain(r"(?P<name>\w+)");

        assert_eq!(
            lines,
            vec![
                r#"(?P<name>...) captures a match into the named group "name""#,
                r#"  \w matches any word character; repeat the previous token one or more times"#,
            ]
        );
    }

    #[test]
    fn attaches_quantifiers_to_classes_and_escapes() {
        assert_eq!(
            explain(r"[a-z]+\d{2}"),
            vec![
                "[a-z] matches one character present in the set; repeat the previous token one or more times",
                r"\d matches any digit, equivalent to [0-9]; repeat the previous token exactly 2 times",
            ]
        );
    }
}
