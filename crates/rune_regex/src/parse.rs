use crate::ast::RegexExpr;

pub fn parse_regex(pattern: &str) -> Result<RegexExpr, String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    let mut captures = 0usize;
    let expr = parse_alt(&chars, &mut i, &mut captures)?;
    if i != chars.len() {
        return Err(format!("Unexpected '{}'", chars[i]));
    }
    let names = crate::ast::named_captures(&expr);
    let mut map: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (name, idx) in names {
        map.entry(name).or_default().push(idx);
    }
    resolve_named(expr, &map)
}

fn resolve_named(
    expr: RegexExpr,
    map: &std::collections::HashMap<String, Vec<usize>>,
) -> Result<RegexExpr, String> {
    Ok(match expr {
        RegexExpr::NamedBackref(name) => match map.get(&name) {
            Some(indices) if indices.len() == 1 => RegexExpr::Backref(indices[0] + 1),
            Some(indices) => RegexExpr::BackrefMulti(indices.iter().map(|i| *i + 1).collect()),
            None => return Err(format!("Invalid named capture referenced: {name}")),
        },
        RegexExpr::BackrefMulti(_) => expr,
        RegexExpr::Group(inner, cap, name) => {
            RegexExpr::Group(Box::new(resolve_named(*inner, map)?), cap, name)
        }
        RegexExpr::Concat(parts) => RegexExpr::Concat(
            parts
                .into_iter()
                .map(|p| resolve_named(p, map))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        RegexExpr::Alt(a, b) => RegexExpr::Alt(
            Box::new(resolve_named(*a, map)?),
            Box::new(resolve_named(*b, map)?),
        ),
        RegexExpr::Star(e) => RegexExpr::Star(Box::new(resolve_named(*e, map)?)),
        RegexExpr::Plus(e) => RegexExpr::Plus(Box::new(resolve_named(*e, map)?)),
        RegexExpr::Optional(e) => RegexExpr::Optional(Box::new(resolve_named(*e, map)?)),
        RegexExpr::Lookahead { expr, negated } => RegexExpr::Lookahead {
            expr: Box::new(resolve_named(*expr, map)?),
            negated,
        },
        other => other,
    })
}

fn parse_alt(chars: &[char], i: &mut usize, captures: &mut usize) -> Result<RegexExpr, String> {
    let mut left = parse_seq(chars, i, captures)?;
    while *i < chars.len() && chars[*i] == '|' {
        *i += 1;
        let right = parse_seq(chars, i, captures)?;
        left = RegexExpr::Alt(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_seq(chars: &[char], i: &mut usize, captures: &mut usize) -> Result<RegexExpr, String> {
    let mut nodes = Vec::new();
    while *i < chars.len() && chars[*i] != '|' && chars[*i] != ')' {
        nodes.push(parse_repetition(chars, i, captures)?);
    }
    Ok(if nodes.is_empty() {
        RegexExpr::Empty
    } else if nodes.len() == 1 {
        nodes.remove(0)
    } else {
        RegexExpr::Concat(nodes)
    })
}

fn parse_repetition(
    chars: &[char],
    i: &mut usize,
    captures: &mut usize,
) -> Result<RegexExpr, String> {
    let node = parse_atom(chars, i, captures)?;
    if *i < chars.len() {
        match chars[*i] {
            '*' => {
                *i += 1;
                return Ok(RegexExpr::Star(Box::new(node)));
            }
            '+' => {
                *i += 1;
                return Ok(RegexExpr::Plus(Box::new(node)));
            }
            '?' => {
                *i += 1;
                return Ok(RegexExpr::Optional(Box::new(node)));
            }
            '{' => {
                // {n}, {n,}, {n,m} quantifier. If the braces don't enclose a
                // valid count, `{` is a literal (non-unicode mode).
                let save = *i;
                *i += 1;
                let mut n = 0usize;
                let mut digits = 0usize;
                while *i < chars.len() && chars[*i].is_ascii_digit() {
                    n = n * 10 + (chars[*i] as u8 - b'0') as usize;
                    digits += 1;
                    *i += 1;
                }
                if digits == 0 {
                    *i = save;
                    return Ok(node);
                }
                let mut m = n;
                let has_m = if *i < chars.len() && chars[*i] == ',' {
                    *i += 1;
                    m = 0;
                    let mut m_digits = 0usize;
                    while *i < chars.len() && chars[*i].is_ascii_digit() {
                        m = m * 10 + (chars[*i] as u8 - b'0') as usize;
                        m_digits += 1;
                        *i += 1;
                    }
                    if m_digits > 0 {
                        true
                    } else {
                        // {n,} — unbounded upper
                        if *i < chars.len() && chars[*i] == '}' {
                            *i += 1;
                            let mut nodes = vec![node.clone(); n];
                            if n > 0 {
                                nodes.push(RegexExpr::Star(Box::new(node)));
                            }
                            return Ok(if nodes.len() == 1 {
                                nodes.remove(0)
                            } else {
                                RegexExpr::Concat(nodes)
                            });
                        }
                        *i = save;
                        return Ok(node);
                    }
                } else {
                    false
                };
                if *i >= chars.len() || chars[*i] != '}' {
                    *i = save;
                    return Ok(node);
                }
                *i += 1;
                if has_m && m < n {
                    return Err("Invalid quantifier range".into());
                }
                if m == n {
                    let mut nodes = vec![node.clone(); n];
                    return Ok(if nodes.len() == 1 {
                        nodes.remove(0)
                    } else {
                        RegexExpr::Concat(nodes)
                    });
                }
                // {n,m}: n copies + (m - n) optional copies.
                let mut nodes: Vec<RegexExpr> = Vec::new();
                for _ in 0..n {
                    nodes.push(node.clone());
                }
                for _ in n..m {
                    nodes.push(RegexExpr::Optional(Box::new(node.clone())));
                }
                return Ok(RegexExpr::Concat(nodes));
            }
            _ => {}
        }
    }
    Ok(node)
}

fn parse_atom(chars: &[char], i: &mut usize, captures: &mut usize) -> Result<RegexExpr, String> {
    if *i >= chars.len() {
        return Err("Unexpected end of pattern".into());
    }
    match chars[*i] {
        '.' => {
            *i += 1;
            Ok(RegexExpr::Dot)
        }
        '^' => {
            *i += 1;
            Ok(RegexExpr::AnchorStart)
        }
        '$' => {
            *i += 1;
            Ok(RegexExpr::AnchorEnd)
        }
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
                        's' => vec![
                            (' ', ' '),
                            ('\t', '\t'),
                            ('\n', '\n'),
                            ('\r', '\r'),
                            ('\x0C', '\x0C'),
                        ],
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
                'b' | 'B' => Ok(RegexExpr::WordBoundary { negated: c == 'B' }),
                'k' => {
                    if *i < chars.len() && chars[*i] == '<' {
                        *i += 1;
                        let start = *i;
                        while *i < chars.len() && chars[*i] != '>' {
                            *i += 1;
                        }
                        if *i >= chars.len() {
                            return Err("Unterminated named backreference".into());
                        }
                        let name: String = chars[start..*i].iter().collect();
                        *i += 1;
                        if name.is_empty() {
                            return Err("Empty named backreference".into());
                        }
                        Ok(RegexExpr::NamedBackref(name))
                    } else {
                        Ok(RegexExpr::Literal('k'))
                    }
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
            // Lookahead assertions: (?=...) positive, (?!...) negative.
            if *i + 1 < chars.len()
                && chars[*i] == '?'
                && (chars[*i + 1] == '=' || chars[*i + 1] == '!')
            {
                let negated = chars[*i + 1] == '!';
                *i += 2;
                let expr = parse_alt(chars, i, captures)?;
                if *i >= chars.len() || chars[*i] != ')' {
                    return Err("Unclosed group".into());
                }
                *i += 1;
                return Ok(RegexExpr::Lookahead {
                    expr: Box::new(expr),
                    negated,
                });
            }
            // Check for (?:...) non-capturing group and (?<name>...) named.
            let mut name: Option<String> = None;
            let capturing = if *i + 1 < chars.len() && chars[*i] == '?' && chars[*i + 1] == ':' {
                *i += 2;
                false
            } else if *i + 1 < chars.len() && chars[*i] == '?' && chars[*i + 1] == '<' {
                if *i + 2 < chars.len() && (chars[*i + 2] == '=' || chars[*i + 2] == '!') {
                    return Err("Lookbehind is not supported".into());
                }
                *i += 2;
                let start = *i;
                while *i < chars.len() && chars[*i] != '>' {
                    *i += 1;
                }
                if *i >= chars.len() {
                    return Err("Unterminated group name".into());
                }
                let n: String = chars[start..*i].iter().collect();
                *i += 1;
                if n.is_empty()
                    || !n
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
                {
                    return Err("Invalid capture group name".into());
                }
                name = Some(n);
                true
            } else {
                true
            };
            let cap_idx = if capturing {
                let idx = *captures;
                *captures += 1;
                Some(idx)
            } else {
                None
            };
            let expr = parse_alt(chars, i, captures)?;
            if *i >= chars.len() || chars[*i] != ')' {
                return Err("Unclosed group".into());
            }
            *i += 1;
            Ok(RegexExpr::Group(Box::new(expr), cap_idx, name))
        }
        ')' | '|' | '*' | '+' | '?' => Err(format!("Unexpected '{}'", chars[*i])),
        c => {
            *i += 1;
            Ok(RegexExpr::Literal(c))
        }
    }
}

fn parse_char_class(chars: &[char], i: &mut usize) -> Result<RegexExpr, String> {
    let negated = *i < chars.len() && chars[*i] == '^';
    if negated {
        *i += 1;
    }
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
        assert_eq!(
            r,
            RegexExpr::Concat(vec![
                RegexExpr::Literal('a'),
                RegexExpr::Literal('b'),
                RegexExpr::Literal('c'),
            ])
        );
    }

    #[test]
    fn test_alt() {
        let r = parse_regex("a|b").unwrap();
        assert_eq!(
            r,
            RegexExpr::Alt(
                Box::new(RegexExpr::Literal('a')),
                Box::new(RegexExpr::Literal('b')),
            )
        );
    }

    #[test]
    fn test_star() {
        let r = parse_regex("a*").unwrap();
        assert_eq!(r, RegexExpr::Star(Box::new(RegexExpr::Literal('a'))));
    }
}
