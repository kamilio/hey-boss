//! Bounded database timing evidence. Never retain SQL values or result rows.
use super::wire::Command;
use std::time::Instant;

#[derive(Clone)]
pub(super) struct Operation {
    pub name: &'static str,
    pub sql: String,
}

impl Operation {
    pub fn new(command: &Command) -> Self {
        let (name, sql) = match command {
            Command::Hello => ("hello", ""),
            Command::CheckSchema => ("schema", ""),
            Command::ExclusiveSession => ("exclusive session", ""),
            Command::ReadTransaction => ("read transaction", ""),
            Command::Prepare { sql } => ("prepare", sql.as_str()),
            Command::Execute { sql, .. } => ("execute", sql.as_str()),
            Command::Query { sql, .. } => ("query", sql.as_str()),
            Command::Batch { sql } => ("batch", sql.as_str()),
            Command::Backup { .. } => ("backup", ""),
        };
        Self {
            name,
            sql: redact(sql),
        }
    }
}

fn redact(sql: &str) -> String {
    let mut output = String::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if output.len() >= 192 {
            output.push('…');
            break;
        }
        match c {
            '\'' | '"' | '`' | '[' => {
                let end = if c == '[' { ']' } else { c };
                while let Some(next) = chars.next() {
                    if next == end {
                        if chars.peek() == Some(&end) {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
                output.push('?');
            }
            '-' if chars.peek() == Some(&'-') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
                output.push(' ');
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
                output.push(' ');
            }
            '0'..='9' => {
                while chars
                    .peek()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
                {
                    chars.next();
                }
                output.push('?');
            }
            c if c.is_whitespace() || c.is_control() => {
                if !output.ends_with(' ') {
                    output.push(' ');
                }
            }
            c => output.push(c),
        }
    }
    output
}

pub(super) struct Activity {
    pub session: usize,
    pub acquired: Instant,
    pub started: Instant,
    pub operation: Operation,
    pub executing: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_exclude_values_comments_paths_and_control_characters() {
        let operation = Operation::new(&Command::Query {
            sql: "SELECT id FROM work WHERE body='private''text' AND n=1234 /* private comment */ -- private line\nAND name=\"private identifier\" AND x=`private name` AND y=[private bracket]".into(),
            values: vec![super::super::wire::SqlValue::Text("private binding".into())],
        });
        assert_eq!(operation.name, "query");
        assert!(!operation.sql.contains("private"));
        assert!(!operation.sql.contains("1234"));
        assert!(!operation.sql.contains('\n'));
        assert!(operation.sql.contains("SELECT id FROM work WHERE body=?"));
        assert!(
            Operation::new(&Command::Backup {
                path: "/private/path".into()
            })
            .sql
            .is_empty()
        );
        assert!(redact(&"SELECT data FROM work ".repeat(100)).len() <= 199);
    }
}
