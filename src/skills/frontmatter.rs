//! SKILL.md frontmatter 解析与生成。
//!
//! 采用极简 YAML 风格（只支持单行键值，不引入 YAML 依赖）：
//!
//! ```text
//! ---
//! name: 示例技能
//! description: 一句话说明
//! version: 0.1
//! triggers: git, rebase
//! ---
//! 正文...
//! ```

/// 技能元数据（SKILL.md frontmatter）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frontmatter {
    pub name: String,
    pub description: String,
    pub version: String,
    pub triggers: Vec<String>,
}

/// 解析 SKILL.md，返回 `(元数据, 正文)`。
///
/// 无 frontmatter 时用正文首行启发式填充 name/description，正文原样返回。
pub fn parse(content: &str) -> (Frontmatter, String) {
    let trimmed = content.strip_prefix('\u{feff}').unwrap_or(content);
    if let Some(rest) = strip_open(trimmed) {
        if let Some((head, tail)) = split_close(rest) {
            // 分隔行后的第一个换行不属于正文
            let body = tail.strip_prefix('\n').unwrap_or(tail);
            return (parse_fields(head), body.to_string());
        }
    }
    heuristic(trimmed)
}

/// 生成 SKILL.md 全文。满足 `parse(compose(fm, body)) == (fm, body)`（正文原样保留）。
pub fn compose(fm: &Frontmatter, body: &str) -> String {
    let triggers = fm
        .triggers
        .iter()
        .map(|t| sanitize(t))
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("name: {}\n", sanitize(&fm.name)));
    out.push_str(&format!("description: {}\n", sanitize(&fm.description)));
    out.push_str(&format!("version: {}\n", sanitize(&fm.version)));
    out.push_str(&format!("triggers: {triggers}\n"));
    out.push_str("---\n");
    out.push_str(body);
    out
}

/// 去掉开头的 `---` 分隔行（兼容 CRLF 与 BOM）。
fn strip_open(s: &str) -> Option<&str> {
    s.strip_prefix("---\r\n")
        .or_else(|| s.strip_prefix("---\n"))
}

/// 找到结束的 `---` 行，返回（头部字段区，分隔行之后的剩余内容）。
fn split_close(rest: &str) -> Option<(&str, &str)> {
    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']).trim_end() == "---" {
            let head = &rest[..offset];
            let tail = &rest[offset + line.len()..];
            return Some((head, tail));
        }
        offset += line.len();
    }
    None
}

/// 解析 `key: value` 行；未知键忽略，缺省 version 为 0.1。
fn parse_fields(head: &str) -> Frontmatter {
    let mut fm = Frontmatter::default();
    let mut has_version = false;
    for line in head.lines() {
        let Some((raw_key, raw_val)) = line.split_once(':') else {
            continue;
        };
        let key = raw_key.trim().to_ascii_lowercase();
        let val = raw_val
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .trim()
            .to_string();
        match key.as_str() {
            "name" => fm.name = val,
            "description" => fm.description = val,
            "version" => {
                has_version = true;
                fm.version = val;
            }
            "triggers" => fm.triggers = split_triggers(&val),
            _ => {}
        }
    }
    if !has_version {
        fm.version = "0.1".to_string();
    }
    fm
}

fn split_triggers(val: &str) -> Vec<String> {
    val.trim_matches(|c| c == '[' || c == ']')
        .split(',')
        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 无 frontmatter 时的启发式：首个非空行为名称，第二行为描述（缺省回落为名称）。
fn heuristic(content: &str) -> (Frontmatter, String) {
    let mut lines = content.lines().map(str::trim).filter(|l| !l.is_empty());
    let name = clean_heading(lines.next().unwrap_or(""));
    let description = lines
        .next()
        .map(clean_heading)
        .unwrap_or_else(|| name.clone());
    let fm = Frontmatter {
        name,
        description,
        version: "0.1".to_string(),
        triggers: Vec::new(),
    };
    (fm, content.to_string())
}

/// 去掉 Markdown 标题符号
fn clean_heading(line: &str) -> String {
    line.trim_start_matches('#').trim().to_string()
}

/// frontmatter 值必须是单行
fn sanitize(value: &str) -> String {
    value.replace(['\r', '\n'], " ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_then_parse_round_trip() {
        let fm = Frontmatter {
            name: "Git 专家".to_string(),
            description: "处理分支、rebase 与冲突".to_string(),
            version: "0.2".to_string(),
            triggers: vec!["git".to_string(), "rebase".to_string()],
        };
        let body = "第一行\n\n```rust\nfn main() {}\n```\n";
        let text = compose(&fm, body);
        let (parsed_fm, parsed_body) = parse(&text);
        assert_eq!(parsed_fm, fm);
        assert_eq!(parsed_body, body);
    }

    #[test]
    fn round_trip_with_default_frontmatter() {
        let fm = Frontmatter::default();
        let (parsed_fm, parsed_body) = parse(&compose(&fm, "正文"));
        assert_eq!(parsed_fm, fm);
        assert_eq!(parsed_body, "正文");
    }

    #[test]
    fn parses_crlf_and_optional_keys() {
        let text = "---\r\nname: x\r\nversion: 1.0\r\n---\r\nbody";
        let (fm, body) = parse(text);
        assert_eq!(fm.name, "x");
        assert_eq!(fm.version, "1.0");
        assert_eq!(fm.description, "");
        assert_eq!(fm.triggers, Vec::<String>::new());
        assert_eq!(body, "body");
    }

    #[test]
    fn without_frontmatter_uses_first_lines() {
        let content = "# 我的技能\n\n这是说明行\n正文内容";
        let (fm, body) = parse(content);
        assert_eq!(fm.name, "我的技能");
        assert_eq!(fm.description, "这是说明行");
        assert_eq!(fm.version, "0.1");
        assert_eq!(body, content);
    }

    #[test]
    fn single_line_content_falls_back_to_name_as_description() {
        let (fm, body) = parse("只有一行");
        assert_eq!(fm.name, "只有一行");
        assert_eq!(fm.description, "只有一行");
        assert_eq!(body, "只有一行");
    }
}
