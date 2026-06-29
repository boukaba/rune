use crate::ast::RegexExpr;

pub fn parse_regex(pattern: &str) -> Result<RegexExpr, String> {
    let mut chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    parse_alt(&chars, &mut i)
}

fn parse_alt(chars: &[char], i: &mut usize) -> Result<RegexExpr, String> {
    let mut left = parse_seq(chars, i)?;
    while *i < chars.len() && chars[*i] == '|' {
        *i += 1;
        let right = parse_seq(chars, i)?;
        left = RegexExpr::Alt(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_seq(chars: &[char], i: &mut usize) -> Result<RegexExpr, String> {
    let mut nodes = Vec::new();
    while *i < chars.len() && chars[*i] != '|' && chars[*i] != ')' {
        nodes.push(parse_repetition(chars, i)?);
    }
    Ok(if nodes.is_empty() {
        RegexExpr::Empty
    } else if nodes.len() == 1 {
        nodes.remove(0)
    } else {
        RegexExpr::Concat(nodes)
    })
}

fn parse_repetition(chars: &[char], i: &mut usize) -> Result<RegexExpr, String> {
    let node = parse_atom(chars, i)?;
    if *i < chars.len() {
        match chars[*i] {
            '*' => { *i += 1; return Ok(RegexExpr::Star(Box::new(node))); }
            '+' => { *i += 1; return Ok(RegexExpr::Plus(Box::new(node))); }
            '?' => { *i += 1; return Ok(RegexExpr::Optional(Box::new(node))); }
            _ => {}
        }
    }
    Ok(node)
}

fn parse_atom(chars: &[char], i: &mut usize) -> Result<RegexExpr, String> {
    if *i >= chars.len() {
        return Err("Unexpected end of pattern".into());
    }
    match chars[*i] {
        '.' => { *i += 1; Ok(RegexExpr::Dot) }
        '^' => { *i += 1; Ok(RegexExpr::AnchorStart) }
        '$' => { *i += 1; Ok(RegexExpr::AnchorEnd) }
        '\\' => {
            *i += 1;
            if *i >= chars.len() {
                return Err("Trailing backslash".into());
            }
            let c = chars[*i];
            *i += 1;
            match c {
                'd' | 'D' | 'w' | 'W' | 's' | 'S' => {
                    let negated = c.is_uppercase();
                    let ranges = match c.to_ascii_lowercase() {
                        'd' => vec![('0', '9')],
                        'w' => vec![('0', '9'), ('A', 'Z'), ('a', 'z'), ('_', '_')],
                        's' => vec![(' ', ' '), ('\t', '\t'), ('\n', '\n'), ('\r', '\r'), ('\x0C', '\x0C')],
                        _ => unreachable!(),
                    };
                    Ok(RegexExpr::CharClass { negated, ranges })
                }
                '0'..='9' => {
                    let n = c as u8 - b'0';
                    if n == 0 {
                        Ok(RegexExpr::Literal('\0'))
                    } else {
                        Ok(RegexExpr::Backref(n as usize))
                    }
                }
                'b' | 'B' => {
                    // Word boundary — simplified: skip
                    Ok(RegexExpr::Empty)
                }
                'n' => Ok(RegexExpr::Literal('\n')),
                'r' => Ok(RegexExpr::Literal('\r')),
                't' => Ok(RegexExpr::Literal('\t')),
                'f' => Ok(RegexExpr::Literal('\x0C')),
                'v' => Ok(RegexExpr::Literal('\x0B')),
                _ => Ok(RegexExpr::Literal(c)),
            }
        }
        '[' => {
            *i += 1;
            parse_char_class(chars, i)
        }
        '(' => {
            *i += 1;
            // Check for (?:...) non-capturing group
            let capturing = if *i + 1 < chars.len() && chars[*i] == '?' && chars[*i + 1] == ':' {
                *i += 2;
                false
            } else {
                true
            };
            let expr = parse_alt(chars, i)?;
            if *i >= chars.len() || chars[*i] != ')' {
                return Err("Unclosed group".into());
            }
            *i += 1;
            Ok(RegexExpr::Group(Box::new(expr), if capturing { Some(0) } else { None }))
        }
        ')' | '|' | '*' | '+' | '?' => {
            Err(format!("Unexpected '{}'", chars[*i]))
        }
        c => {
            *i += 1;
            Ok(RegexExpr::Literal(c))
        }
    }
}

fn parse_char_class(chars: &[char], i: &mut usize) -> Result<RegexExpr, String> {
    let negated = *i < chars.len() && chars[*i] == '^';
    if negated { *i += 1; }
    let mut ranges = Vec::new();
    loop {
        if *i >= chars.len() {
            return Err("Unclosed character class".into());
        }
        if chars[*i] == ']' {
            *i += 1;
            break;
        }
        let start = chars[*i];
        *i += 1;
        if *i < chars.len() && chars[*i] == '-' {
            *i += 1;
            if *i >= chars.len() || chars[*i] == ']' {
                ranges.push((start, start));
                ranges.push(('-', '-'));
            } else {
                let end = chars[*i];
                *i += 1;
                ranges.push((start, end));
            }
        } else {
            ranges.push((start, start));
        }
    }
    Ok(RegexExpr::CharClass { negated, ranges })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literal() {
        let r = parse_regex("abc").unwrap();
        assert_eq!(r, RegexExpr::Concat(vec![
            RegexExpr::Literal('a'),
            RegexExpr::Literal('b'),
            RegexExpr::Literal('c'),
        ]));
    }

    #[test]
    fn test_alt() {
        let r = parse_regex("a|b").unwrap();
        assert_eq!(r, RegexExpr::Alt(
            Box::new(RegexExpr::Literal('a')),
            Box::new(RegexExpr::Literal('b')),
        ));
    }

    #[test]
    fn test_star() {
        let r = parse_regex("a*").unwrap();
        assert_eq!(r, RegexExpr::Star(Box::new(RegexExpr::Literal('a'))));
    }
}
