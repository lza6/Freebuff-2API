//! roster 注入：把技能目录（名称 + 描述）包装成 system 前缀片段。

/// 用 `[freebuff-skills]` 标签包裹 roster；prefix 为空时返回空串（调用方可直接拼接）。
pub fn roster_block(prefix: &str) -> String {
    if prefix.trim().is_empty() {
        return String::new();
    }
    format!("\n\n[freebuff-skills]\n{prefix}\n[/freebuff-skills]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_prefix_yields_empty_block() {
        assert_eq!(roster_block(""), "");
        assert_eq!(roster_block("   \n"), "");
    }

    #[test]
    fn wraps_non_empty_prefix() {
        let block = roster_block("### Git 专家\n处理分支与 rebase");
        assert!(block.starts_with("\n\n[freebuff-skills]\n"));
        assert!(block.ends_with("\n[/freebuff-skills]"));
        assert!(block.contains("### Git 专家"));
    }
}
